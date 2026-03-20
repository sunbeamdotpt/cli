use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sunbeam configuration stored at ~/.sunbeam.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SunbeamConfig {
    #[serde(default)]
    pub production_host: String,
    #[serde(default)]
    pub infra_directory: String,
    #[serde(default)]
    pub acme_email: String,
}

fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sunbeam.json")
}

/// Load configuration from ~/.sunbeam.json, return default if not found.
pub fn load_config() -> SunbeamConfig {
    let path = config_path();
    if !path.exists() {
        return SunbeamConfig::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            crate::output::warn(&format!(
                "Failed to parse config from {}: {e}",
                path.display()
            ));
            SunbeamConfig::default()
        }),
        Err(e) => {
            crate::output::warn(&format!(
                "Failed to read config from {}: {e}",
                path.display()
            ));
            SunbeamConfig::default()
        }
    }
}

/// Save configuration to ~/.sunbeam.json.
pub fn save_config(config: &SunbeamConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create config directory: {}",
                parent.display()
            )
        })?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to save config to {}", path.display()))?;
    crate::output::ok(&format!("Configuration saved to {}", path.display()));
    Ok(())
}

/// Get production host from config or SUNBEAM_SSH_HOST environment variable.
pub fn get_production_host() -> String {
    let config = load_config();
    if !config.production_host.is_empty() {
        return config.production_host;
    }
    std::env::var("SUNBEAM_SSH_HOST").unwrap_or_default()
}

/// Get infrastructure directory from config.
pub fn get_infra_directory() -> String {
    load_config().infra_directory
}

/// Infrastructure manifests directory as a Path.
///
/// Prefers the configured infra_directory; falls back to a path relative to
/// the current executable (works when running from the development checkout).
pub fn get_infra_dir() -> PathBuf {
    let configured = load_config().infra_directory;
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    // Dev fallback: walk up from the executable to find monorepo root
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| {
            let mut dir = p.as_path();
            for _ in 0..10 {
                dir = dir.parent()?;
                if dir.join("infrastructure").is_dir() {
                    return Some(dir.join("infrastructure"));
                }
            }
            None
        })
        .unwrap_or_else(|| PathBuf::from("infrastructure"))
}

/// Monorepo root directory (parent of the infrastructure directory).
pub fn get_repo_root() -> PathBuf {
    get_infra_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Clear configuration file.
pub fn clear_config() -> Result<()> {
    let path = config_path();
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        crate::output::ok(&format!(
            "Configuration cleared from {}",
            path.display()
        ));
    } else {
        crate::output::warn("No configuration file found to clear");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SunbeamConfig::default();
        assert!(config.production_host.is_empty());
        assert!(config.infra_directory.is_empty());
        assert!(config.acme_email.is_empty());
    }

    #[test]
    fn test_config_roundtrip() {
        let config = SunbeamConfig {
            production_host: "user@example.com".to_string(),
            infra_directory: "/path/to/infra".to_string(),
            acme_email: "ops@example.com".to_string(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let loaded: SunbeamConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.production_host, "user@example.com");
        assert_eq!(loaded.infra_directory, "/path/to/infra");
        assert_eq!(loaded.acme_email, "ops@example.com");
    }
}
