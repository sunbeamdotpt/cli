use anyhow::{Context, Result};
use std::path::PathBuf;

static KUSTOMIZE_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kustomize"));
static HELM_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/helm"));

fn cache_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("sunbeam")
        .join("bin")
}

/// Extract an embedded binary to the cache directory if not already present.
fn extract_embedded(data: &[u8], name: &str) -> Result<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create cache dir: {}", dir.display()))?;

    let dest = dir.join(name);

    // Skip if already extracted and same size
    if dest.exists() {
        if let Ok(meta) = std::fs::metadata(&dest) {
            if meta.len() == data.len() as u64 {
                return Ok(dest);
            }
        }
    }

    std::fs::write(&dest, data)
        .with_context(|| format!("Failed to write {}", dest.display()))?;

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
