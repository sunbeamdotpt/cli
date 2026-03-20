use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

/// Compile-time commit SHA set by build.rs.
pub const COMMIT: &str = env!("SUNBEAM_COMMIT");

/// Compile-time build target triple set by build.rs.
pub const TARGET: &str = env!("SUNBEAM_TARGET");

/// Compile-time build date set by build.rs.
pub const BUILD_DATE: &str = env!("SUNBEAM_BUILD_DATE");

/// Artifact name prefix for this platform.
fn artifact_name() -> String {
    format!("sunbeam-{TARGET}")
}

/// Resolve the forge URL (Gitea instance).
///
/// TODO: Once kube.rs exposes `get_domain()`, derive this automatically as
/// `https://src.{domain}`. For now we read the SUNBEAM_FORGE_URL environment
/// variable with a sensible fallback.
fn forge_url() -> String {
    if let Ok(url) = std::env::var("SUNBEAM_FORGE_URL") {
        return url.trim_end_matches('/').to_string();
    }

    // Derive from production_host domain in config
    let config = crate::config::load_config();
    if !config.production_host.is_empty() {
        // production_host is like "user@server.example.com" — extract domain
        let host = config
            .production_host
            .split('@')
            .last()
            .unwrap_or(&config.production_host);
        // Strip any leading subdomain segments that look like a hostname to get the base domain.
        // For a host like "admin.sunbeam.pt", the forge is "src.sunbeam.pt".
        // Heuristic: use the last two segments as the domain.
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() >= 2 {
            let domain = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
            return format!("https://src.{domain}");
        }
    }

    // Hard fallback — will fail at runtime if not configured, which is fine.
    String::new()
}

/// Cache file location for background update checks.
fn update_cache_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".local/share"))
        .join("sunbeam")
        .join("update-check.json")
}

// ---------------------------------------------------------------------------
// Gitea API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BranchResponse {
    commit: BranchCommit,
}

#[derive(Debug, Deserialize)]
struct BranchCommit {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ArtifactListResponse {
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    name: String,
    id: u64,
}

// ---------------------------------------------------------------------------
// Update-check cache
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    last_check: DateTime<Utc>,
    latest_commit: String,
    current_commit: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Print version information.
pub fn cmd_version() {
    println!("sunbeam {COMMIT}");
    println!("  target: {TARGET}");
    println!("  built:  {BUILD_DATE}");
}

/// Self-update from the latest mainline commit via Gitea CI artifacts.
pub async fn cmd_update() -> Result<()> {
    let base = forge_url();
    if base.is_empty() {
        bail!(
            "Forge URL not configured. Set SUNBEAM_FORGE_URL or configure a \
             production host via `sunbeam config set --host`."
        );
    }

    crate::output::step("Checking for updates...");

    let client = reqwest::Client::new();

    // 1. Check latest commit on mainline
    let latest_commit = fetch_latest_commit(&client, &base).await?;
    let short_latest = &latest_commit[..std::cmp::min(8, latest_commit.len())];

    crate::output::ok(&format!("Current: {COMMIT}"));
    crate::output::ok(&format!("Latest:  {short_latest}"));

    if latest_commit.starts_with(COMMIT) || COMMIT.starts_with(&latest_commit[..std::cmp::min(COMMIT.len(), latest_commit.len())]) {
        crate::output::ok("Already up to date.");
        return Ok(());
    }

    // 2. Find the CI artifact for our platform
    crate::output::step("Downloading update...");
    let wanted = artifact_name();

    let artifacts = fetch_artifacts(&client, &base).await?;
    let binary_artifact = artifacts
        .iter()
        .find(|a| a.name == wanted)
        .with_context(|| format!("No artifact found for platform '{wanted}'"))?;

    let checksums_artifact = artifacts
        .iter()
        .find(|a| a.name == "checksums.txt" || a.name == "checksums");

    // 3. Download the binary
    let binary_url = format!(
        "{base}/api/v1/repos/studio/cli/actions/artifacts/{id}",
        id = binary_artifact.id
    );
    let binary_bytes = client
        .get(&binary_url)
        .send()
        .await?
        .error_for_status()
        .context("Failed to download binary artifact")?
        .bytes()
        .await?;

    crate::output::ok(&format!("Downloaded {} bytes", binary_bytes.len()));

    // 4. Verify SHA256 if checksums artifact exists
    if let Some(checksums) = checksums_artifact {
        let checksums_url = format!(
            "{base}/api/v1/repos/studio/cli/actions/artifacts/{id}",
            id = checksums.id
        );
        let checksums_text = client
            .get(&checksums_url)
            .send()
            .await?
            .error_for_status()
            .context("Failed to download checksums")?
            .text()
            .await?;

        verify_checksum(&binary_bytes, &wanted, &checksums_text)?;
        crate::output::ok("SHA256 checksum verified.");
    } else {
        crate::output::warn("No checksums artifact found; skipping verification.");
    }

    // 5. Atomic self-replace
    crate::output::step("Installing update...");
    let current_exe = std::env::current_exe().context("Failed to determine current executable path")?;
    atomic_replace(&current_exe, &binary_bytes)?;

    crate::output::ok(&format!(
        "Updated sunbeam {COMMIT} -> {short_latest}"
    ));

    // Update the cache so background check knows we are current
    let _ = write_cache(&UpdateCache {
        last_check: Utc::now(),
        latest_commit: latest_commit.clone(),
        current_commit: latest_commit,
    });

    Ok(())
}

/// Background update check. Returns a notification message if a newer version
/// is available, or None if up-to-date / on error / checked too recently.
///
/// This function never blocks for long and never returns errors — it silently
/// returns None on any failure.
pub async fn check_update_background() -> Option<String> {
    // Read cache
    let cache_path = update_cache_path();
    if let Ok(data) = fs::read_to_string(&cache_path) {
        if let Ok(cache) = serde_json::from_str::<UpdateCache>(&data) {
            let age = Utc::now().signed_duration_since(cache.last_check);
            if age.num_seconds() < 3600 {
                // Checked recently — just compare cached values
                if cache.latest_commit.starts_with(COMMIT)
                    || COMMIT.starts_with(&cache.latest_commit[..std::cmp::min(COMMIT.len(), cache.latest_commit.len())])
                {
                    return None; // up to date
                }
                let short = &cache.latest_commit[..std::cmp::min(8, cache.latest_commit.len())];
                return Some(format!(
                    "A newer version of sunbeam is available ({short}). Run `sunbeam update` to upgrade."
                ));
            }
        }
    }

    // Time to check again
    let base = forge_url();
    if base.is_empty() {
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    let latest = fetch_latest_commit(&client, &base).await.ok()?;

    let cache = UpdateCache {
        last_check: Utc::now(),
        latest_commit: latest.clone(),
        current_commit: COMMIT.to_string(),
    };
    let _ = write_cache(&cache);

    if latest.starts_with(COMMIT)
        || COMMIT.starts_with(&latest[..std::cmp::min(COMMIT.len(), latest.len())])
    {
        return None;
    }

    let short = &latest[..std::cmp::min(8, latest.len())];
    Some(format!(
        "A newer version of sunbeam is available ({short}). Run `sunbeam update` to upgrade."
    ))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Fetch the latest commit SHA on the mainline branch.
async fn fetch_latest_commit(client: &reqwest::Client, forge_url: &str) -> Result<String> {
    let url = format!("{forge_url}/api/v1/repos/studio/cli/branches/mainline");
    let resp: BranchResponse = client
        .get(&url)
        .send()
        .await?
        .error_for_status()
        .context("Failed to query mainline branch")?
        .json()
        .await?;
    Ok(resp.commit.id)
}

/// Fetch the list of CI artifacts for the repo.
async fn fetch_artifacts(client: &reqwest::Client, forge_url: &str) -> Result<Vec<Artifact>> {
    let url = format!("{forge_url}/api/v1/repos/studio/cli/actions/artifacts");
    let resp: ArtifactListResponse = client
        .get(&url)
        .send()
        .await?
        .error_for_status()
        .context("Failed to query CI artifacts")?
        .json()
        .await?;
    Ok(resp.artifacts)
}

/// Verify that the downloaded binary matches the expected SHA256 from checksums text.
///
/// Checksums file format (one per line):
///   <hex-sha256>  <filename>
fn verify_checksum(binary: &[u8], artifact_name: &str, checksums_text: &str) -> Result<()> {
    let actual = {
        let mut hasher = Sha256::new();
        hasher.update(binary);
        format!("{:x}", hasher.finalize())
    };

    for line in checksums_text.lines() {
        // Split on whitespace — format is "<hash>  <name>" or "<hash> <name>"
        let mut parts = line.split_whitespace();
        if let (Some(expected_hash), Some(name)) = (parts.next(), parts.next()) {
            if name == artifact_name {
                if actual != expected_hash {
                    bail!(
                        "Checksum mismatch for {artifact_name}:\n  expected: {expected_hash}\n  actual:   {actual}"
                    );
                }
                return Ok(());
            }
        }
    }

    bail!("No checksum entry found for '{artifact_name}' in checksums file");
}

/// Atomically replace the binary at `target` with `new_bytes`.
///
/// Writes to a temp file in the same directory, sets executable permissions,
/// then renames over the original.
fn atomic_replace(target: &std::path::Path, new_bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .context("Cannot determine parent directory of current executable")?;

    let tmp_path = parent.join(".sunbeam-update.tmp");

    // Write new binary
    fs::write(&tmp_path, new_bytes).context("Failed to write temporary update file")?;

    // Set executable permissions (unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))
            .context("Failed to set executable permissions")?;
    }

    // Atomic rename
    fs::rename(&tmp_path, target).context("Failed to replace current executable")?;

    Ok(())
}

/// Write the update-check cache to disk.
fn write_cache(cache: &UpdateCache) -> Result<()> {
    let path = update_cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cache)?;
    fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_consts() {
        // COMMIT, TARGET, BUILD_DATE are set at compile time
        assert!(!COMMIT.is_empty());
        assert!(!TARGET.is_empty());
        assert!(!BUILD_DATE.is_empty());
    }

    #[test]
    fn test_artifact_name() {
        let name = artifact_name();
        assert!(name.starts_with("sunbeam-"));
        assert!(name.contains(TARGET));
    }

    #[test]
    fn test_verify_checksum_ok() {
        let data = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = format!("{:x}", hasher.finalize());

        let checksums = format!("{hash}  sunbeam-test");
        assert!(verify_checksum(data, "sunbeam-test", &checksums).is_ok());
    }

    #[test]
    fn test_verify_checksum_mismatch() {
        let checksums = "0000000000000000000000000000000000000000000000000000000000000000  sunbeam-test";
        assert!(verify_checksum(b"hello", "sunbeam-test", checksums).is_err());
    }

    #[test]
    fn test_verify_checksum_missing_entry() {
        let checksums = "abcdef1234567890  sunbeam-other";
        assert!(verify_checksum(b"hello", "sunbeam-test", checksums).is_err());
    }

    #[test]
    fn test_update_cache_path() {
        let path = update_cache_path();
        assert!(path.to_string_lossy().contains("sunbeam"));
        assert!(path.to_string_lossy().ends_with("update-check.json"));
    }

    #[test]
    fn test_cache_roundtrip() {
        let cache = UpdateCache {
            last_check: Utc::now(),
            latest_commit: "abc12345".to_string(),
            current_commit: "def67890".to_string(),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let loaded: UpdateCache = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.latest_commit, "abc12345");
        assert_eq!(loaded.current_commit, "def67890");
    }
}
