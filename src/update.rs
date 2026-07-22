//! Self-update from GitHub tagged releases.

use sdk::bail;
use sdk::error::{Result, ResultExt};
use sdk::info;
use sdk::reqwest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;

/// Compile-time commit SHA set by build.rs.
pub const COMMIT: &str = env!("SUNBEAM_COMMIT");

/// Compile-time build target triple set by build.rs.
pub const TARGET: &str = env!("SUNBEAM_TARGET");

/// Compile-time build date set by build.rs.
pub const BUILD_DATE: &str = env!("SUNBEAM_BUILD_DATE");

/// Compile-time package version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub API base URL — threaded through for testability (wiremock).
const GITHUB_API_BASE: &str = "https://api.github.com";

/// GitHub repository that publishes the release assets.
const GITHUB_REPO: &str = "sunbeamdotpt/cli";

/// Raw binary asset name for this platform.
fn asset_name() -> String {
    format!("sunbeam-raw-{TARGET}")
}

// ---------------------------------------------------------------------------
// GitHub API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
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

/// Self-update from the latest GitHub release.
#[tracing::instrument(skip(logger))]
pub async fn cmd_update(logger: &sdk::logger::Logger) -> Result<()> {
    let current_exe = std::env::current_exe().ctx("Failed to determine current executable path")?;
    run_update(logger, GITHUB_API_BASE, &current_exe).await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Full update flow against a given GitHub API base, replacing `current_exe`.
async fn run_update(
    logger: &sdk::logger::Logger,
    api_base: &str,
    current_exe: &std::path::Path,
) -> Result<()> {
    info!(logger, "Checking for updates...");

    let client = reqwest::Client::new();

    // 1. Fetch the latest release
    let release = fetch_latest_release(&client, api_base).await?;
    let latest = tag_version(&release.tag_name);

    info!(logger, "Current version", version = VERSION);
    info!(logger, "Latest version", version = latest);

    if latest == VERSION {
        info!(logger, "Already up to date.");
        return Ok(());
    }

    // 2. Select the raw binary asset for our platform
    let wanted = asset_name();
    let binary_asset = find_asset(&release, &wanted)?;
    let checksums_asset = find_asset(&release, "checksums.txt")?;

    // 3. Download checksums first, then the binary
    info!(logger, "Downloading update...", version = latest);
    let checksums_text = client
        .get(&checksums_asset.browser_download_url)
        .send()
        .await?
        .error_for_status()
        .ctx("Failed to download checksums.txt")?
        .text()
        .await?;

    let binary_bytes = client
        .get(&binary_asset.browser_download_url)
        .send()
        .await?
        .error_for_status()
        .ctx("Failed to download binary asset")?
        .bytes()
        .await?;

    info!(logger, "Downloaded bytes", size = binary_bytes.len());

    // 4. Verify SHA256 — mismatch is a hard error, no replacement
    verify_checksum(&binary_bytes, &wanted, &checksums_text)?;
    info!(logger, "SHA256 checksum verified.");

    // 5. Atomic self-replace
    info!(logger, "Installing update...");
    atomic_replace(current_exe, &binary_bytes)?;

    info!(logger, "Updated sunbeam", old = VERSION, new = latest);
    Ok(())
}

/// Fetch the latest release from the GitHub API.
///
/// GitHub requires a User-Agent header on all API requests.
async fn fetch_latest_release(client: &reqwest::Client, api_base: &str) -> Result<Release> {
    let url = format!("{api_base}/repos/{GITHUB_REPO}/releases/latest");
    client
        .get(&url)
        .header(
            reqwest::header::USER_AGENT,
            format!("sunbeam-cli/{VERSION}"),
        )
        .send()
        .await?
        .error_for_status()
        .ctx("Failed to query latest GitHub release")?
        .json()
        .await
        .ctx("Failed to parse latest GitHub release")
}

/// Strip the leading `v` from a release tag to get the bare version.
fn tag_version(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Find a release asset by exact name.
fn find_asset<'a>(release: &'a Release, name: &str) -> Result<&'a ReleaseAsset> {
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .with_ctx(|| format!("Release '{}' has no asset named '{name}'", release.tag_name))
}

/// Verify that the downloaded binary matches the expected SHA256 from checksums text.
///
/// Checksums file format (one per line):
///   <hex-sha256>  <filename>
fn verify_checksum(binary: &[u8], asset_name: &str, checksums_text: &str) -> Result<()> {
    let actual = {
        let mut hasher = Sha256::new();
        hasher.update(binary);
        format!("{:x}", hasher.finalize())
    };

    for line in checksums_text.lines() {
        // Split on whitespace — format is "<hash>  <name>" or "<hash> <name>"
        let mut parts = line.split_whitespace();
        if let (Some(expected_hash), Some(name)) = (parts.next(), parts.next())
            && name == asset_name
        {
            if actual != expected_hash {
                bail!(
                    "Checksum mismatch for {asset_name}:\n  expected: {expected_hash}\n  actual:   {actual}"
                );
            }
            return Ok(());
        }
    }

    bail!("No checksum entry found for '{asset_name}' in checksums file");
}

/// Atomically replace the binary at `target` with `new_bytes`.
///
/// Writes to a temp file in the same directory, sets executable permissions,
/// then renames over the original.
fn atomic_replace(target: &std::path::Path, new_bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .ctx("Cannot determine parent directory of current executable")?;

    let tmp_path = parent.join(".sunbeam-update.tmp");

    // Write new binary
    fs::write(&tmp_path, new_bytes).ctx("Failed to write temporary update file")?;

    // Set executable permissions (unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))
            .ctx("Failed to set executable permissions")?;
    }

    // Atomic rename
    fs::rename(&tmp_path, target).ctx("Failed to replace current executable")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_BINARY: &[u8] = b"new-sunbeam-binary";

    fn test_logger() -> sdk::logger::Logger {
        sdk::logger::Logger::new(sdk::logger::TracingSink)
    }

    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Release JSON with our platform asset + checksums.txt, both pointing at
    /// the mock server, plus a distractor asset for another platform.
    fn release_json(server: &MockServer, tag: &str) -> serde_json::Value {
        let uri = server.uri();
        let target_asset = asset_name();
        serde_json::json!({
            "tag_name": tag,
            "assets": [
                {
                    "name": format!("sunbeam_{tag}_aarch64-apple-darwin.tar.gz"),
                    "browser_download_url": format!("{uri}/download/pkg.tar.gz")
                },
                {
                    "name": "sunbeam-raw-x86_64-unknown-linux-gnu",
                    "browser_download_url": format!("{uri}/download/other-binary")
                },
                {
                    "name": target_asset,
                    "browser_download_url": format!("{uri}/download/binary")
                },
                {
                    "name": "checksums.txt",
                    "browser_download_url": format!("{uri}/download/checksums.txt")
                }
            ]
        })
    }

    /// Mount the standard happy-path mocks: release, checksums, binary.
    async fn mount_release_flow(server: &MockServer, tag: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/{GITHUB_REPO}/releases/latest")))
            .and(header("user-agent", format!("sunbeam-cli/{VERSION}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(release_json(server, tag)))
            .mount(server)
            .await;
    }

    #[test]
    fn test_version_consts() {
        // COMMIT, TARGET, BUILD_DATE are set at compile time
        assert!(!COMMIT.is_empty());
        assert!(!TARGET.is_empty());
        assert!(!BUILD_DATE.is_empty());
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_asset_name() {
        let name = asset_name();
        assert!(name.starts_with("sunbeam-raw-"));
        assert!(name.contains(TARGET));
    }

    #[test]
    fn test_tag_version_strips_v() {
        assert_eq!(tag_version("v3.0.0"), "3.0.0");
        assert_eq!(tag_version("3.0.0"), "3.0.0");
        assert_eq!(tag_version("v"), "");
    }

    #[test]
    fn test_find_asset_selects_exact_name() {
        let release: Release = serde_json::from_value(serde_json::json!({
            "tag_name": "v1.2.3",
            "assets": [
                {"name": "sunbeam-raw-x86_64-apple-darwin", "browser_download_url": "https://x/a"},
                {"name": "sunbeam-raw-aarch64-apple-darwin", "browser_download_url": "https://x/b"}
            ]
        }))
        .unwrap();
        let asset = find_asset(&release, "sunbeam-raw-aarch64-apple-darwin").unwrap();
        assert_eq!(asset.browser_download_url, "https://x/b");
    }

    #[test]
    fn test_find_asset_missing_errors() {
        let release: Release = serde_json::from_value(serde_json::json!({
            "tag_name": "v1.2.3",
            "assets": [
                {"name": "checksums.txt", "browser_download_url": "https://x/c"}
            ]
        }))
        .unwrap();
        let err = find_asset(&release, "sunbeam-raw-aarch64-apple-darwin").unwrap_err();
        assert!(
            err.to_string().contains("sunbeam-raw-aarch64-apple-darwin"),
            "err: {err}"
        );
        assert!(err.to_string().contains("v1.2.3"), "err: {err}");
    }

    #[test]
    fn test_verify_checksum_ok() {
        let checksums = format!("{}  sunbeam-test", sha256_hex(b"hello world"));
        assert!(verify_checksum(b"hello world", "sunbeam-test", &checksums).is_ok());
    }

    #[test]
    fn test_verify_checksum_mismatch() {
        let checksums =
            "0000000000000000000000000000000000000000000000000000000000000000  sunbeam-test";
        assert!(verify_checksum(b"hello", "sunbeam-test", checksums).is_err());
    }

    #[test]
    fn test_verify_checksum_missing_entry() {
        let checksums = "abcdef1234567890  sunbeam-other";
        assert!(verify_checksum(b"hello", "sunbeam-test", checksums).is_err());
    }

    #[tokio::test]
    async fn test_fetch_latest_release_parses_response() {
        let server = MockServer::start().await;
        mount_release_flow(&server, "v9.9.9").await;

        let release = fetch_latest_release(&reqwest::Client::new(), &server.uri())
            .await
            .unwrap();
        assert_eq!(release.tag_name, "v9.9.9");
        assert_eq!(release.assets.len(), 4);
        assert!(release.assets.iter().any(|a| a.name == asset_name()));
    }

    #[tokio::test]
    async fn test_fetch_latest_release_http_error_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/repos/{GITHUB_REPO}/releases/latest")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let err = fetch_latest_release(&reqwest::Client::new(), &server.uri())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("GitHub release"), "err: {err}");
    }

    #[tokio::test]
    async fn test_run_update_happy_path() {
        let server = MockServer::start().await;
        mount_release_flow(&server, "v9.9.9").await;

        let checksums = format!("{}  {}\n", sha256_hex(TEST_BINARY), asset_name());
        Mock::given(method("GET"))
            .and(path("/download/checksums.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string(checksums))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/binary"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(TEST_BINARY))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("sunbeam");
        std::fs::write(&exe, b"old-binary").unwrap();

        run_update(&test_logger(), &server.uri(), &exe)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), TEST_BINARY);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o111,
                0o111,
                "expected executable bits, got {mode:o}"
            );
        }
    }

    #[tokio::test]
    async fn test_run_update_checksum_mismatch_refuses_replacement() {
        let server = MockServer::start().await;
        mount_release_flow(&server, "v9.9.9").await;

        let checksums = format!("{}  {}\n", "0".repeat(64), asset_name());
        Mock::given(method("GET"))
            .and(path("/download/checksums.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string(checksums))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/download/binary"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(TEST_BINARY))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("sunbeam");
        std::fs::write(&exe, b"old-binary").unwrap();

        let err = run_update(&test_logger(), &server.uri(), &exe)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Checksum mismatch"), "err: {err}");
        // Original binary must be untouched.
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-binary");
    }

    #[tokio::test]
    async fn test_run_update_missing_platform_asset_errors() {
        let server = MockServer::start().await;
        // Release without our platform's raw asset.
        let uri = server.uri();
        Mock::given(method("GET"))
            .and(path(format!("/repos/{GITHUB_REPO}/releases/latest")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name": "v9.9.9",
                "assets": [
                    {
                        "name": "checksums.txt",
                        "browser_download_url": format!("{uri}/download/checksums.txt")
                    }
                ]
            })))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("sunbeam");
        std::fs::write(&exe, b"old-binary").unwrap();

        let err = run_update(&test_logger(), &server.uri(), &exe)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(&asset_name()), "err: {err}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-binary");
    }

    #[tokio::test]
    async fn test_run_update_already_up_to_date() {
        let server = MockServer::start().await;
        // Tag matches the running version — no asset mocks on purpose; any
        // asset request would 404 and fail the update.
        mount_release_flow(&server, &format!("v{VERSION}")).await;

        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("sunbeam");
        std::fs::write(&exe, b"old-binary").unwrap();

        run_update(&test_logger(), &server.uri(), &exe)
            .await
            .unwrap();

        // Untouched.
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-binary");
    }

    #[test]
    fn test_atomic_replace_writes_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("sunbeam");
        std::fs::write(&target, b"old-binary").unwrap();

        atomic_replace(&target, b"new-binary").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new-binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o111,
                0o111,
                "expected executable bits, got {mode:o}"
            );
        }
        // Temp file must not linger.
        assert!(!tmp.path().join(".sunbeam-update.tmp").exists());
    }

    #[test]
    fn test_atomic_replace_missing_directory_errors() {
        let target = std::path::Path::new("no-such-dir-xyz/target-binary");
        let err = atomic_replace(target, b"x").unwrap_err();
        assert!(err.to_string().contains("temporary"), "err: {err}");
    }
}
