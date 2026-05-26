//! Runtime binary download and caching (kustomize, helm).

use crate::error::{Result, ResultExt};
use std::io::Read;
use std::path::PathBuf;

const KUSTOMIZE_VERSION: &str = "v5.8.1";
const HELM_VERSION: &str = "v4.1.0";

/// Legacy bin cache dir — used only for migration.
fn legacy_cache_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("sunbeam")
        .join("bin")
}

fn cache_dir() -> PathBuf {
    let new_dir = crate::config::sunbeam_dir().join("bin");

    // Migration: copy binaries from legacy location if new dir doesn't exist yet
    if !new_dir.exists() {
        let legacy = legacy_cache_dir();
        if legacy.is_dir() {
            let _ = std::fs::create_dir_all(&new_dir);
            if let Ok(entries) = std::fs::read_dir(&legacy) {
                for entry in entries.flatten() {
                    let dest = new_dir.join(entry.file_name());
                    let _ = std::fs::copy(entry.path(), &dest);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
                    }
                }
            }
        }
    }

    new_dir
}

fn current_platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => other,
    };

    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };

    (os, arch)
}

fn download_url(tool: &str, version: &str, os: &str, arch: &str) -> String {
    match tool {
        "kustomize" => format!(
            "https://github.com/kubernetes-sigs/kustomize/releases/download/\
             kustomize%2F{version}/kustomize_{version}_{os}_{arch}.tar.gz"
        ),
        "helm" => format!("https://get.helm.sh/helm-{version}-{os}-{arch}.tar.gz"),
        _ => panic!("Unknown tool: {tool}"),
    }
}

fn archive_entry_path(tool: &str, os: &str, arch: &str) -> String {
    match tool {
        "kustomize" => "kustomize".to_string(),
        "helm" => format!("{os}-{arch}/helm"),
        _ => unreachable!(),
    }
}

/// Download and extract a tool to the cache directory if not already present.
fn ensure_tool(tool: &str, version: &str) -> Result<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)
        .with_ctx(|| format!("Failed to create cache dir: {}", dir.display()))?;

    let dest = dir.join(tool);

    // Skip if already present
    if dest.exists() {
        return Ok(dest);
    }

    let (os, arch) = current_platform();
    let url = download_url(tool, version, os, arch);
    let entry_path = archive_entry_path(tool, os, arch);

    tracing::info!("Downloading {tool} {version} for {os}/{arch}...");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .ctx("Failed to build HTTP client")?;

    let response = client
        .get(&url)
        .send()
        .with_ctx(|| format!("Failed to download {tool} from {url}"))?;
    tracing::debug!("downloaded {tool} {version} ({size} bytes)", size = response.content_length().unwrap_or(0));
    let bytes = response
        .bytes()
        .with_ctx(|| format!("Failed to read {tool} response"))?;

    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().ctx("Failed to read tar entries")? {
        let mut entry = entry.ctx("Failed to read tar entry")?;
        let path = entry.path().ctx("Failed to read entry path")?.to_path_buf();
        if path.to_string_lossy() == entry_path {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).ctx("Failed to read binary")?;
            std::fs::write(&dest, &data)
                .with_ctx(|| format!("Failed to write {}", dest.display()))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                    .ctx("Failed to set permissions")?;
            }

            tracing::info!("Installed {tool} ({size} bytes)", size = data.len());
            return Ok(dest);
        }
    }

    Err(crate::error::SunbeamError::Other(format!(
        "Could not find {entry_path} in {tool} archive"
    )))
}

/// Ensure kustomize is downloaded and return its path.
pub fn ensure_kustomize() -> Result<PathBuf> {
    ensure_tool("kustomize", KUSTOMIZE_VERSION)
}

/// Ensure helm is downloaded and return its path.
pub fn ensure_helm() -> Result<PathBuf> {
    ensure_tool("helm", HELM_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_ends_with_sunbeam_bin() {
        let dir = cache_dir();
        assert!(
            dir.ends_with(".sunbeam/bin"),
            "cache_dir() should end with .sunbeam/bin, got: {}",
            dir.display()
        );
    }

    #[test]
    fn cache_dir_is_absolute() {
        let dir = cache_dir();
        assert!(
            dir.is_absolute(),
            "cache_dir() should return an absolute path, got: {}",
            dir.display()
        );
    }

    #[test]
    fn ensure_kustomize_returns_valid_path() {
        let path = ensure_kustomize().expect("ensure_kustomize should succeed");
        assert!(
            path.ends_with("kustomize"),
            "ensure_kustomize path should end with 'kustomize', got: {}",
            path.display()
        );
        assert!(
            path.exists(),
            "kustomize binary should exist at: {}",
            path.display()
        );
    }

    #[test]
    fn ensure_helm_returns_valid_path() {
        let path = ensure_helm().expect("ensure_helm should succeed");
        assert!(
            path.ends_with("helm"),
            "ensure_helm path should end with 'helm', got: {}",
            path.display()
        );
        assert!(
            path.exists(),
            "helm binary should exist at: {}",
            path.display()
        );
    }

    #[test]
    fn ensure_kustomize_is_idempotent() {
        let path1 = ensure_kustomize().expect("first call should succeed");
        let path2 = ensure_kustomize().expect("second call should succeed");
        assert_eq!(
            path1, path2,
            "ensure_kustomize should return the same path on repeated calls"
        );
    }

    #[test]
    fn ensure_helm_is_idempotent() {
        let path1 = ensure_helm().expect("first call should succeed");
        let path2 = ensure_helm().expect("second call should succeed");
        assert_eq!(
            path1, path2,
            "ensure_helm should return the same path on repeated calls"
        );
    }

    #[test]
    fn downloaded_kustomize_is_executable() {
        let path = ensure_kustomize().expect("ensure_kustomize should succeed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&path)
                .expect("should read metadata")
                .permissions();
            assert!(
                perms.mode() & 0o111 != 0,
                "kustomize binary should be executable"
            );
        }
    }

    #[test]
    fn downloaded_helm_is_executable() {
        let path = ensure_helm().expect("ensure_helm should succeed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&path)
                .expect("should read metadata")
                .permissions();
            assert!(
                perms.mode() & 0o111 != 0,
                "helm binary should be executable"
            );
        }
    }
}
