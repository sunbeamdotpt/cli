use crate::error::{Result, ResultExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Config data model
// ---------------------------------------------------------------------------

/// Sunbeam configuration stored at ~/.sunbeam.json.
///
/// Supports kubectl-style named contexts. Each context bundles a domain,
/// kube context, SSH host, and infrastructure directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SunbeamConfig {
    /// The active context name. If empty, uses "default".
    #[serde(default, rename = "current-context")]
    pub current_context: String,

    /// Named contexts.
    #[serde(default)]
    pub contexts: HashMap<String, Context>,

    // --- Legacy fields (migrated on load) ---
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub production_host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub infra_directory: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub acme_email: String,
}

/// A named context — everything needed to target a specific environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Context {
    /// The domain suffix (e.g. "sunbeam.pt", "192.168.105.3.sslip.io").
    #[serde(default)]
    pub domain: String,

    /// Kubernetes context name (e.g. "production", "sunbeam").
    #[serde(default, rename = "kube-context")]
    pub kube_context: String,

    /// SSH host for production tunnel (e.g. "sienna@62.210.145.138").
    #[serde(default, rename = "ssh-host")]
    pub ssh_host: String,

    /// Infrastructure directory root.
    #[serde(default, rename = "infra-dir")]
    pub infra_dir: String,

    /// ACME email for cert-manager.
    #[serde(default, rename = "acme-email")]
    pub acme_email: String,

    /// VPN coordination server URL (Headscale). When set, `sunbeam connect`
    /// can establish a WireGuard tunnel through this server and the CLI
    /// will route k8s API traffic through it instead of falling back to
    /// SSH or kubeconfig.
    #[serde(default, rename = "vpn-url", skip_serializing_if = "String::is_empty")]
    pub vpn_url: String,

    /// VPN pre-auth key for registering with the coordination server.
    /// Stored in plain text — keep this file readable only by the user.
    #[serde(
        default,
        rename = "vpn-auth-key",
        skip_serializing_if = "String::is_empty"
    )]
    pub vpn_auth_key: String,

    /// Hostname of the cluster API server peer to look up in the netmap.
    /// When set, the VPN daemon resolves this against the netmap's peer
    /// list and proxies k8s API traffic to that peer's tailnet IP. When
    /// empty, falls back to a static fallback address.
    #[serde(
        default,
        rename = "vpn-cluster-host",
        skip_serializing_if = "String::is_empty"
    )]
    pub vpn_cluster_host: String,

    /// Headscale API key for `sunbeam vpn create-key` and other admin
    /// commands. Generated once via `headscale apikeys create`. Stored
    /// in plain text — keep this file readable only by the user.
    #[serde(
        default,
        rename = "vpn-api-key",
        skip_serializing_if = "String::is_empty"
    )]
    pub vpn_api_key: String,

    /// Skip TLS certificate verification when talking to the VPN
    /// coordination server (control plane, DERP relay, REST API).
    /// Only set this for test stacks with self-signed certs — leave
    /// false for production.
    #[serde(default, rename = "vpn-tls-insecure", skip_serializing_if = "is_false")]
    pub vpn_tls_insecure: bool,

    /// Cluster DNS server (`host:port`) reachable through the tunnel.
    /// Typically `10.43.0.10:53` for k3s CoreDNS. Empty disables
    /// domain-name resolution in the SOCKS proxy — only literal IPs
    /// are then allowed as CONNECT destinations.
    #[serde(
        default,
        rename = "vpn-dns-server",
        skip_serializing_if = "String::is_empty"
    )]
    pub vpn_dns_server: String,

    /// Comma-separated DNS search domains appended to bare names
    /// that have no dot. Defaults to
    /// `svc.cluster.local,cluster.local` when empty.
    #[serde(
        default,
        rename = "vpn-dns-search",
        skip_serializing_if = "String::is_empty"
    )]
    pub vpn_dns_search: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------------------
// Active context (set once at startup, read everywhere)
// ---------------------------------------------------------------------------

static ACTIVE_CONTEXT: OnceLock<Context> = OnceLock::new();

/// Initialize the active context. Called once from cli::dispatch().
pub fn set_active_context(ctx: Context) {
    let _ = ACTIVE_CONTEXT.set(ctx);
}

/// Get the active context. Panics if not initialized (should never happen
/// after dispatch starts).
pub fn active_context() -> &'static Context {
    ACTIVE_CONTEXT
        .get()
        .expect("active context not initialized")
}

/// Get the domain from the active context. Returns empty string if not set.
pub fn domain() -> &'static str {
    ACTIVE_CONTEXT
        .get()
        .map(|c| c.domain.as_str())
        .unwrap_or("")
}

// ---------------------------------------------------------------------------
// Central path helpers — all sunbeam state lives under ~/.sunbeam/
// ---------------------------------------------------------------------------

/// Base directory for all sunbeam state: ~/.sunbeam/
pub fn sunbeam_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sunbeam")
}

/// Context-specific directory: ~/.sunbeam/{context}/
pub fn context_dir(context_name: &str) -> PathBuf {
    let name = if context_name.is_empty() {
        "default"
    } else {
        context_name
    };
    sunbeam_dir().join(name)
}

// ---------------------------------------------------------------------------
// Config file I/O
// ---------------------------------------------------------------------------

fn config_path() -> PathBuf {
    sunbeam_dir().join("config.json")
}

/// Legacy config path (~/.sunbeam.json) — used only for migration.
fn legacy_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".sunbeam.json")
}

/// Load configuration, return default if not found.
/// Migrates legacy ~/.sunbeam.json → ~/.sunbeam/config.json on first load.
/// Migrates legacy flat config to context-based format.
pub fn load_config() -> SunbeamConfig {
    let path = config_path();

    // Migration: move legacy ~/.sunbeam.json → ~/.sunbeam/config.json
    if !path.exists() {
        let legacy = legacy_config_path();
        if legacy.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(&legacy, &path).is_ok() {
                let _ = std::fs::remove_file(&legacy);
                crate::output::ok(&format!(
                    "Migrated config: {} → {}",
                    legacy.display(),
                    path.display()
                ));
            }
        }
    }

    if !path.exists() {
        return SunbeamConfig::default();
    }
    let mut config: SunbeamConfig = match std::fs::read_to_string(&path) {
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
    };

    // Migrate legacy flat fields into a "production" context
    if !config.production_host.is_empty() && !config.contexts.contains_key("production") {
        let domain = derive_domain_from_host(&config.production_host);
        config.contexts.insert(
            "production".to_string(),
            Context {
                domain,
                kube_context: "production".to_string(),
                ssh_host: config.production_host.clone(),
                infra_dir: config.infra_directory.clone(),
                acme_email: config.acme_email.clone(),
                ..Default::default()
            },
        );
        if config.current_context.is_empty() {
            config.current_context = "production".to_string();
        }
    }

    config
}

/// Save configuration to ~/.sunbeam/config.json.
pub fn save_config(config: &SunbeamConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_ctx(|| format!("Failed to create config directory: {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)
        .with_ctx(|| format!("Failed to save config to {}", path.display()))?;
    crate::output::ok(&format!("Configuration saved to {}", path.display()));
    Ok(())
}

/// Resolve the context to use, given CLI flags and config.
///
/// Priority (same as kubectl):
///   1. `--context` flag (explicit context name)
///   2. `current-context` from config
///   3. Default to "local"
pub fn resolve_context(
    config: &SunbeamConfig,
    _env_flag: &str,
    context_override: Option<&str>,
    domain_override: &str,
) -> Context {
    let context_name = if let Some(explicit) = context_override {
        explicit.to_string()
    } else if !config.current_context.is_empty() {
        config.current_context.clone()
    } else {
        "local".to_string()
    };

    let mut ctx = config
        .contexts
        .get(&context_name)
        .cloned()
        .unwrap_or_else(|| {
            // Synthesize defaults for well-known names
            match context_name.as_str() {
                "local" => Context {
                    kube_context: "sunbeam".to_string(),
                    ..Default::default()
                },
                "production" => Context {
                    kube_context: "production".to_string(),
                    ssh_host: config.production_host.clone(),
                    infra_dir: config.infra_directory.clone(),
                    acme_email: config.acme_email.clone(),
                    domain: derive_domain_from_host(&config.production_host),
                    ..Default::default()
                },
                _ => Default::default(),
            }
        });

    // CLI flags override context values
    if !domain_override.is_empty() {
        ctx.domain = domain_override.to_string();
    }

    ctx
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive a domain from an SSH host (e.g. "user@admin.sunbeam.pt" → "sunbeam.pt").
fn derive_domain_from_host(host: &str) -> String {
    let raw = host.split('@').last().unwrap_or(host);
    let raw = raw.split(':').next().unwrap_or(raw);
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        String::new()
    }
}

/// Get production host from config or SUNBEAM_SSH_HOST environment variable.
pub fn get_production_host() -> String {
    let config = load_config();
    // Check active context first
    if let Some(ctx) = ACTIVE_CONTEXT.get() {
        if !ctx.ssh_host.is_empty() {
            return ctx.ssh_host.clone();
        }
    }
    if !config.production_host.is_empty() {
        return config.production_host;
    }
    std::env::var("SUNBEAM_SSH_HOST").unwrap_or_default()
}

/// Infrastructure manifests directory as a Path.
pub fn get_infra_dir() -> PathBuf {
    // Check active context
    if let Some(ctx) = ACTIVE_CONTEXT.get() {
        if !ctx.infra_dir.is_empty() {
            return PathBuf::from(&ctx.infra_dir);
        }
    }
    let configured = load_config().infra_directory;
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    // Dev fallback
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
        std::fs::remove_file(&path).with_ctx(|| format!("Failed to remove {}", path.display()))?;
        crate::output::ok(&format!("Configuration cleared from {}", path.display()));
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
        assert!(config.current_context.is_empty());
        assert!(config.contexts.is_empty());
    }

    #[test]
    fn test_derive_domain_from_host() {
        assert_eq!(
            derive_domain_from_host("sienna@admin.sunbeam.pt"),
            "sunbeam.pt"
        );
        assert_eq!(derive_domain_from_host("user@62.210.145.138"), "145.138");
        assert_eq!(derive_domain_from_host("sunbeam.pt"), "sunbeam.pt");
        assert_eq!(derive_domain_from_host("localhost"), "");
    }

    #[test]
    fn test_legacy_migration() {
        let json = r#"{
            "production_host": "sienna@62.210.145.138",
            "infra_directory": "/path/to/infra",
            "acme_email": "ops@sunbeam.pt"
        }"#;
        let config: SunbeamConfig = serde_json::from_str(json).unwrap();
        // After load_config migration, contexts would be populated.
        // Here we just test the struct deserializes legacy fields.
        assert_eq!(config.production_host, "sienna@62.210.145.138");
        assert!(config.contexts.is_empty()); // migration happens in load_config()
    }

    #[test]
    fn test_context_roundtrip() {
        let mut config = SunbeamConfig::default();
        config.current_context = "production".to_string();
        config.contexts.insert(
            "production".to_string(),
            Context {
                domain: "sunbeam.pt".to_string(),
                kube_context: "production".to_string(),
                ssh_host: "sienna@server.sunbeam.pt".to_string(),
                infra_dir: "/home/infra".to_string(),
                acme_email: "ops@sunbeam.pt".to_string(),
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&config).unwrap();
        let loaded: SunbeamConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.current_context, "production");
        let ctx = loaded.contexts.get("production").unwrap();
        assert_eq!(ctx.domain, "sunbeam.pt");
        assert_eq!(ctx.ssh_host, "sienna@server.sunbeam.pt");
    }

    #[test]
    fn test_resolve_context_explicit_flag() {
        let mut config = SunbeamConfig::default();
        config.contexts.insert(
            "production".to_string(),
            Context {
                domain: "sunbeam.pt".to_string(),
                kube_context: "production".to_string(),
                ..Default::default()
            },
        );
        // --context production explicitly selects the named context
        let ctx = resolve_context(&config, "", Some("production"), "");
        assert_eq!(ctx.domain, "sunbeam.pt");
        assert_eq!(ctx.kube_context, "production");
    }

    #[test]
    fn test_resolve_context_current_context() {
        let mut config = SunbeamConfig::default();
        config.current_context = "staging".to_string();
        config.contexts.insert(
            "staging".to_string(),
            Context {
                domain: "staging.example.com".to_string(),
                ..Default::default()
            },
        );
        // No --context flag, uses current-context
        let ctx = resolve_context(&config, "", None, "");
        assert_eq!(ctx.domain, "staging.example.com");
    }

    #[test]
    fn test_resolve_context_domain_override() {
        let config = SunbeamConfig::default();
        let ctx = resolve_context(&config, "", None, "custom.example.com");
        assert_eq!(ctx.domain, "custom.example.com");
    }

    #[test]
    fn test_resolve_context_defaults_local() {
        let config = SunbeamConfig::default();
        // No current-context, no --context flag → defaults to "local"
        let ctx = resolve_context(&config, "", None, "");
        assert_eq!(ctx.kube_context, "sunbeam");
    }

    #[test]
    fn test_resolve_context_flag_overrides_current() {
        let mut config = SunbeamConfig::default();
        config.current_context = "staging".to_string();
        config.contexts.insert(
            "staging".to_string(),
            Context {
                domain: "staging.example.com".to_string(),
                ..Default::default()
            },
        );
        config.contexts.insert(
            "prod".to_string(),
            Context {
                domain: "prod.example.com".to_string(),
                ..Default::default()
            },
        );
        // --context prod overrides current-context "staging"
        let ctx = resolve_context(&config, "", Some("prod"), "");
        assert_eq!(ctx.domain, "prod.example.com");
    }
}
