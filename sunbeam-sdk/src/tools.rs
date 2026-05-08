//! Embedded binary extraction (kustomize, helm).

use crate::error::{Result, ResultExt};
use std::path::PathBuf;

static KUSTOMIZE_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kustomize"));
static HELM_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/helm"));

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

/// Extract an embedded binary to the cache directory if not already present.
fn extract_embedded(data: &[u8], name: &str) -> Result<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)
        .with_ctx(|| format!("Failed to create cache dir: {}", dir.display()))?;

    let dest = dir.join(name);

    // Skip if already extracted and same size
    if dest.exists()
        && let Ok(meta) = std::fs::metadata(&dest)
        && meta.len() == data.len() as u64
    {
        return Ok(dest);
    }

    std::fs::write(&dest, data).with_ctx(|| format!("Failed to write {}", dest.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(dest)
}

/// Ensure kustomize is extracted and return its path.
pub fn ensure_kustomize() -> Result<PathBuf> {
    extract_embedded(KUSTOMIZE_BIN, "kustomize")
}

/// Ensure helm is extracted and return its path.
pub fn ensure_helm() -> Result<PathBuf> {
    extract_embedded(HELM_BIN, "helm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kustomize_bin_is_non_empty() {
        assert!(
            !KUSTOMIZE_BIN.is_empty(),
            "Embedded kustomize binary should not be empty"
        );
    }

    #[test]
    fn helm_bin_is_non_empty() {
        assert!(
            !HELM_BIN.is_empty(),
            "Embedded helm binary should not be empty"
        );
    }

    #[test]
    fn kustomize_bin_has_reasonable_size() {
        // kustomize binary should be at least 1 MB
        assert!(
            KUSTOMIZE_BIN.len() > 1_000_000,
            "Embedded kustomize binary seems too small: {} bytes",
            KUSTOMIZE_BIN.len()
        );
    }

    #[test]
    fn helm_bin_has_reasonable_size() {
        // helm binary should be at least 1 MB
        assert!(
            HELM_BIN.len() > 1_000_000,
            "Embedded helm binary seems too small: {} bytes",
            HELM_BIN.len()
        );
    }

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
    fn extracted_kustomize_is_executable() {
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
    fn extracted_helm_is_executable() {
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
