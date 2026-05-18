//! Context-based configuration file I/O and path helpers.

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
/// kube context, and infrastructure directory.
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
    /// Infra directory.
    pub infra_directory: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    /// Acme email.
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
    /// Vpn auth key.
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
    /// Vpn cluster host.
    pub vpn_cluster_host: String,

    /// Headscale API key for `sunbeam vpn create-key` and other admin
    /// commands. Generated once via `headscale apikeys create`. Stored
    /// in plain text — keep this file readable only by the user.
    #[serde(
        default,
        rename = "vpn-api-key",
        skip_serializing_if = "String::is_empty"
    )]
    /// Vpn api key.
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
    /// Vpn dns server.
    pub vpn_dns_server: String,

    /// Comma-separated DNS search domains appended to bare names
    /// that have no dot. Defaults to
    /// `svc.cluster.local,cluster.local` when empty.
    #[serde(
        default,
        rename = "vpn-dns-search",
        skip_serializing_if = "String::is_empty"
    )]
    /// Vpn dns search.
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

    // One-shot migration: legacy top-level `infra_directory` / `acme_email`
    // get folded into the current context's per-context fields, then cleared.
    // Per-context is the only source of truth going forward.
    if !config.infra_directory.is_empty() || !config.acme_email.is_empty() {
        let ctx_name = if config.current_context.is_empty() {
            "default".to_string()
        } else {
            config.current_context.clone()
        };
        let legacy_infra = std::mem::take(&mut config.infra_directory);
        let legacy_acme = std::mem::take(&mut config.acme_email);
        let ctx = config.contexts.entry(ctx_name.clone()).or_default();
        if ctx.infra_dir.is_empty() && !legacy_infra.is_empty() {
            ctx.infra_dir = legacy_infra;
        }
        if ctx.acme_email.is_empty() && !legacy_acme.is_empty() {
            ctx.acme_email = legacy_acme;
        }
        // Persist the migration silently — next read will be clean.
        let _ = save_config_silent(&config);
        crate::output::warn(&format!(
            "migrated legacy `infra_directory`/`acme_email` into context `{ctx_name}`. \
             Per-context keys are now the only source of truth."
        ));
    }

    config
}

/// Save configuration to ~/.sunbeam/config.json.
pub fn save_config(config: &SunbeamConfig) -> Result<()> {
    save_config_inner(config, true)
}

/// Save without printing the "Configuration saved to …" confirmation.
/// Used by internal flows like the legacy-field migration.
fn save_config_silent(config: &SunbeamConfig) -> Result<()> {
    save_config_inner(config, false)
}

fn save_config_inner(config: &SunbeamConfig, verbose: bool) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_ctx(|| format!("Failed to create config directory: {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, content)
        .with_ctx(|| format!("Failed to save config to {}", path.display()))?;
    if verbose {
        crate::output::ok(&format!("Configuration saved to {}", path.display()));
    }
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

/// Infrastructure manifests directory as a Path.
///
/// Only the active context's `infra-dir` is authoritative. Legacy top-level
/// `infra_directory` was folded into the per-context field on config load;
/// this function never reads it.
pub fn get_infra_dir() -> PathBuf {
    if let Some(ctx) = ACTIVE_CONTEXT.get()
        && !ctx.infra_dir.is_empty()
    {
        return PathBuf::from(&ctx.infra_dir);
    }
    // Dev fallback — useful when running outside a configured context (e.g.,
    // unit tests or `cargo run` before `sunbeam config set`).
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .and_then(|p| {
            let mut dir = p.as_path();
            for _ in 0..10 {
                dir = dir.parent()?;
                if dir.join("infra/sbbb").is_dir() {
                    return Some(dir.join("infra/sbbb"));
                }
            }
            None
        })
        .unwrap_or_else(|| PathBuf::from("infra/sbbb"))
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
    fn test_context_roundtrip() {
        let mut config = SunbeamConfig {
            current_context: "production".to_string(),
            ..Default::default()
        };
        config.contexts.insert(
            "production".to_string(),
            Context {
                domain: "sunbeam.pt".to_string(),
                kube_context: "production".to_string(),
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
        assert_eq!(ctx.kube_context, "production");
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
        let mut config = SunbeamConfig {
            current_context: "staging".to_string(),
            ..Default::default()
        };
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
    fn test_legacy_fields_fold_into_current_context() {
        // Simulate a loaded-but-not-yet-migrated config: top-level legacy
        // fields set, current-context points at a context whose per-context
        // fields are empty.
        let mut config = SunbeamConfig {
            current_context: "production".to_string(),
            infra_directory: "/legacy/infra".to_string(),
            acme_email: "legacy@example.com".to_string(),
            ..Default::default()
        };
        config
            .contexts
            .insert("production".to_string(), Context::default());
        // Run the same migration logic used in load_config().
        let legacy_infra = std::mem::take(&mut config.infra_directory);
        let legacy_acme = std::mem::take(&mut config.acme_email);
        let ctx = config.contexts.entry("production".to_string()).or_default();
        if ctx.infra_dir.is_empty() && !legacy_infra.is_empty() {
            ctx.infra_dir = legacy_infra;
        }
        if ctx.acme_email.is_empty() && !legacy_acme.is_empty() {
            ctx.acme_email = legacy_acme;
        }
        assert_eq!(config.infra_directory, "");
        assert_eq!(config.acme_email, "");
        let ctx = config.contexts.get("production").unwrap();
        assert_eq!(ctx.infra_dir, "/legacy/infra");
        assert_eq!(ctx.acme_email, "legacy@example.com");
    }

    #[test]
    fn test_legacy_fields_do_not_overwrite_non_empty_context() {
        // Per-context values win over legacy top-level values.
        let mut config = SunbeamConfig {
            current_context: "production".to_string(),
            infra_directory: "/legacy/infra".to_string(),
            ..Default::default()
        };
        config.contexts.insert(
            "production".to_string(),
            Context {
                infra_dir: "/per-context/infra".to_string(),
                ..Default::default()
            },
        );
        let legacy_infra = std::mem::take(&mut config.infra_directory);
        let ctx = config.contexts.entry("production".to_string()).or_default();
        if ctx.infra_dir.is_empty() && !legacy_infra.is_empty() {
            ctx.infra_dir = legacy_infra;
        }
        let ctx = config.contexts.get("production").unwrap();
        assert_eq!(ctx.infra_dir, "/per-context/infra");
    }

    #[test]
    fn test_legacy_fields_serialize_out_when_empty() {
        // skip_serializing_if means the top-level keys vanish after migration.
        let config = SunbeamConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("infra_directory"));
        assert!(!json.contains("acme_email"));
    }

    #[test]
    fn test_resolve_context_flag_overrides_current() {
        let mut config = SunbeamConfig {
            current_context: "staging".to_string(),
            ..Default::default()
        };
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
