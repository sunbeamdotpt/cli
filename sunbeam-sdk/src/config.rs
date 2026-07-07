//! Context-based configuration file I/O and path helpers.

use crate::error::{Result, ResultExt};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Profile data model
// ---------------------------------------------------------------------------

/// A named profile — presets and rules that override manifest fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    /// Profile-scoped presets (shadow global presets with the same name).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub presets: HashMap<String, Preset>,

    /// Rules that map resources to shortcut overrides.
    #[serde(default)]
    pub rules: Vec<Rule>,

    /// Namespaces to skip during apply (workflow behavior, not manifest override).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_namespaces: Vec<String>,

    /// Skip Ory identity stack (workflow behavior).
    #[serde(default)]
    pub skip_ory: bool,

    /// Run in serial mode (workflow behavior).
    #[serde(default)]
    pub serial_mode: bool,
}

/// A preset is a reusable bundle of shortcut values.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Preset {
    /// Flat map of shortcut keys → values.
    #[serde(flatten)]
    pub values: HashMap<String, serde_json::Value>,
}

/// A rule targets one resource and applies presets + explicit shortcuts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Resource name (matches `metadata.name` in manifests).
    pub resource: String,

    /// Optional namespace disambiguator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Optional kind disambiguator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Optional preset to expand before applying explicit shortcuts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,

    /// Top-level shortcuts (merged on top of preset).
    #[serde(flatten)]
    pub shortcuts: HashMap<String, serde_json::Value>,

    /// Named container shortcuts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub containers: HashMap<String, ContainerShortcuts>,

    /// Named volume shortcuts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub volumes: HashMap<String, serde_json::Value>,

    /// Environment variable shortcuts (top-level `env` key).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, serde_json::Value>,
}

/// Shortcuts scoped to a named container.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerShortcuts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, serde_json::Value>,
}

/// How a context references a profile: by name, inline, or not at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProfileRef {
    /// Reference a named profile from the top-level `profiles` map.
    Name(String),
    /// An embedded profile object (context-specific, not shared).
    Inline(Profile),
    /// No profile — serialized as absent.
    #[serde(skip)]
    None,
}

impl ProfileRef {
    /// Returns true if this is the `None` variant.
    pub fn is_none(&self) -> bool {
        matches!(self, ProfileRef::None)
    }
}

impl Default for ProfileRef {
    fn default() -> Self {
        ProfileRef::None
    }
}

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

    /// Named profiles shared across contexts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, Profile>,

    /// Global presets shared across profiles.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub presets: HashMap<String, Preset>,

    /// Named workflow targets (local is implicit; these are remote wfe-servers).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub workflow_targets: HashMap<String, WorkflowTarget>,

    /// Default workflow target name. Empty means "local".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_workflow_target: String,

    /// OAuth2 tokens keyed by domain. This is the unified auth store;
    /// legacy per-domain files under ~/.sunbeam/auth/ are migrated here
    /// on first load.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub auth: HashMap<String, AuthTokens>,

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

    /// Kratos admin API base URL. Used by the kanban module to resolve SSO
    /// subjects to email addresses.
    #[serde(
        default,
        rename = "kratos-admin-url",
        skip_serializing_if = "String::is_empty"
    )]
    pub kratos_admin_url: String,

    /// Profile reference: name string, inline object, or omitted.
    #[serde(default, skip_serializing_if = "ProfileRef::is_none")]
    pub profile: ProfileRef,

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

/// A named workflow target — a remote wfe-server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowTarget {
    /// Server URL (e.g. https://builds.sunbeam.pt).
    pub url: String,
}

/// Cached OAuth2 tokens persisted as part of the unified config.
///
/// Tokens are keyed by domain in `SunbeamConfig.auth` so multiple
/// environments can coexist in one config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    /// Access token.
    pub access_token: String,
    /// Refresh token.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refresh_token: String,
    /// Expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// ID token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
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

/// Get the active context. If one has not been initialized yet, a default
/// context is installed; in normal CLI flow dispatch always initializes this
/// before any command runs.
pub fn active_context() -> &'static Context {
    ACTIVE_CONTEXT.get_or_init(Context::default)
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

/// Legacy auth cache directory (~/.sunbeam/auth/) — used only for migration.
fn legacy_auth_dir() -> PathBuf {
    sunbeam_dir().join("auth")
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
                tracing::info!("Migrated config: {} → {}", legacy.display(), path.display());
            }
        }
    }

    if !path.exists() {
        return SunbeamConfig::default();
    }
    let mut config: SunbeamConfig = match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            tracing::error!("Failed to parse config from {}: {e}", path.display());
            SunbeamConfig::default()
        }),
        Err(e) => {
            tracing::error!("Failed to read config from {}: {e}", path.display());
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
        tracing::info!(
            "migrated legacy `infra_directory`/`acme_email` into context `{ctx_name}`. \
             Per-context keys are now the only source of truth."
        );
    }

    // One-shot migration: legacy per-domain auth cache files
    // (~/.sunbeam/auth/{domain}.json) into the unified config.auth map.
    let legacy_auth = legacy_auth_dir();
    if legacy_auth.is_dir() {
        let mut migrated = false;
        if let Ok(entries) = std::fs::read_dir(&legacy_auth) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let domain = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if domain.is_empty() || config.auth.contains_key(&domain) {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let tokens = match serde_json::from_str::<AuthTokens>(&content) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                config.auth.insert(domain, tokens);
                migrated = true;
            }
        }
        if migrated {
            let _ = save_config_silent(&config);
            tracing::info!(
                "migrated legacy auth cache files from {} into config.json",
                legacy_auth.display()
            );
        }
        // Best-effort removal of the now-redundant auth directory.
        let _ = std::fs::remove_dir_all(&legacy_auth);
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

    // Config now contains OAuth tokens; restrict to owner-only access.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms)
            .with_ctx(|| format!("Failed to set permissions on {}", path.display()))?;
    }

    if verbose {
        tracing::info!("Configuration saved to {}", path.display());
    }
    Ok(())
}

/// Read cached OAuth tokens for the given domain, if any.
pub fn get_auth_tokens(domain: &str) -> Option<AuthTokens> {
    let config = load_config();
    config.auth.get(domain).cloned()
}

/// Store (or replace) OAuth tokens for the given domain.
pub fn set_auth_tokens(domain: &str, tokens: &AuthTokens) -> Result<()> {
    let mut config = load_config();
    config.auth.insert(domain.to_string(), tokens.clone());
    save_config_silent(&config)
}

/// Remove cached OAuth tokens for the given domain.
pub fn remove_auth_tokens(domain: &str) -> Result<()> {
    let mut config = load_config();
    config.auth.remove(domain);
    save_config_silent(&config)
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
                    kube_context: crate::constants::LIMA_KUBE_CONTEXT.to_string(),
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
        tracing::info!("Configuration cleared from {}", path.display());
    } else {
        tracing::info!("No configuration file found to clear");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Profile CRUD helpers
// ---------------------------------------------------------------------------

impl SunbeamConfig {
    /// Resolve a `ProfileRef` into an actual `Profile`.
    ///
    /// - `Name(name)` → look up `self.profiles[name]`
    /// - `Inline(profile)` → return a clone
    /// - `None` → return `None`
    pub fn resolve_profile(&self, profile_ref: &ProfileRef) -> Option<Profile> {
        match profile_ref {
            ProfileRef::Name(name) => self.profiles.get(name).cloned(),
            ProfileRef::Inline(profile) => Some(profile.clone()),
            ProfileRef::None => None,
        }
    }

    /// Add or replace a rule in a named profile.
    pub fn add_rule(&mut self, profile_name: &str, rule: Rule) {
        let profile = self.profiles.entry(profile_name.to_string()).or_default();
        // Remove any existing rule for the same resource
        profile.rules.retain(|r| r.resource != rule.resource);
        profile.rules.push(rule);
    }

    /// Remove a rule by resource name from a named profile.
    pub fn remove_rule(&mut self, profile_name: &str, resource: &str) -> bool {
        if let Some(profile) = self.profiles.get_mut(profile_name) {
            let before = profile.rules.len();
            profile.rules.retain(|r| r.resource != resource);
            profile.rules.len() < before
        } else {
            false
        }
    }

    /// Add or replace a preset in a named profile.
    pub fn add_preset(&mut self, profile_name: &str, preset_name: &str, preset: Preset) {
        let profile = self.profiles.entry(profile_name.to_string()).or_default();
        profile.presets.insert(preset_name.to_string(), preset);
    }

    /// Merge values into an existing preset in a named profile.
    pub fn set_preset(
        &mut self,
        profile_name: &str,
        preset_name: &str,
        values: HashMap<String, serde_json::Value>,
    ) {
        let profile = self.profiles.entry(profile_name.to_string()).or_default();
        let preset = profile.presets.entry(preset_name.to_string()).or_default();
        preset.values.extend(values);
    }

    /// Remove a preset from a named profile.
    pub fn remove_preset(&mut self, profile_name: &str, preset_name: &str) -> bool {
        if let Some(profile) = self.profiles.get_mut(profile_name) {
            profile.presets.remove(preset_name).is_some()
        } else {
            false
        }
    }

    /// Deep-copy a named profile to a new name.
    pub fn copy_profile(&mut self, src: &str, dst: &str) -> bool {
        if let Some(src_profile) = self.profiles.get(src).cloned() {
            self.profiles.insert(dst.to_string(), src_profile);
            true
        } else {
            false
        }
    }

    /// Add or replace a global preset.
    pub fn add_global_preset(&mut self, name: &str, preset: Preset) {
        self.presets.insert(name.to_string(), preset);
    }

    /// Remove a global preset.
    pub fn remove_global_preset(&mut self, name: &str) -> bool {
        self.presets.remove(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SunbeamConfig::default();
        assert!(config.current_context.is_empty());
        assert!(config.contexts.is_empty());
        assert!(config.profiles.is_empty());
        assert!(config.presets.is_empty());
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
        assert_eq!(ctx.kube_context, crate::constants::LIMA_KUBE_CONTEXT);
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
    fn test_auth_tokens_roundtrip() {
        let tokens = AuthTokens {
            access_token: "access_abc".to_string(),
            refresh_token: "refresh_xyz".to_string(),
            expires_at: Utc::now(),
            id_token: Some("id_123".to_string()),
        };
        let json = serde_json::to_string(&tokens).unwrap();
        let loaded: AuthTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.access_token, "access_abc");
        assert_eq!(loaded.refresh_token, "refresh_xyz");
        assert_eq!(loaded.id_token, Some("id_123".to_string()));
    }

    #[test]
    fn test_auth_map_serializes_under_top_level_key() {
        let mut config = SunbeamConfig::default();
        config.auth.insert(
            "sunbeam.pt".to_string(),
            AuthTokens {
                access_token: "ory_at_test".to_string(),
                refresh_token: String::new(),
                expires_at: Utc::now(),
                id_token: None,
            },
        );
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"auth\""));
        assert!(json.contains("\"sunbeam.pt\""));
        assert!(json.contains("\"access_token\""));
    }

    #[test]
    fn test_workflow_target_has_no_token_field() {
        let target = WorkflowTarget {
            url: "https://builds.sunbeam.pt".to_string(),
        };
        let json = serde_json::to_string(&target).unwrap();
        assert!(!json.contains("token"));
        assert!(json.contains("url"));
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

    // -----------------------------------------------------------------------
    // ProfileRef tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_profile_ref_name_roundtrip() {
        let pref = ProfileRef::Name("lima".to_string());
        let json = serde_json::to_string(&pref).unwrap();
        assert_eq!(json, "\"lima\"");
        let back: ProfileRef = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ProfileRef::Name(n) if n == "lima"));
    }

    #[test]
    fn test_profile_ref_inline_roundtrip() {
        let profile = Profile {
            rules: vec![Rule {
                resource: "searxng".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: {
                    let mut m = HashMap::new();
                    m.insert("scale".to_string(), serde_json::json!(0));
                    m
                },
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            }],
            ..Default::default()
        };
        let pref = ProfileRef::Inline(profile);
        let json = serde_json::to_value(&pref).unwrap();
        assert!(json.get("rules").is_some());
        let back: ProfileRef = serde_json::from_value(json).unwrap();
        assert!(matches!(back, ProfileRef::Inline(_)));
    }

    #[test]
    fn test_profile_ref_none_serializes_to_nothing() {
        let ctx = Context {
            profile: ProfileRef::None,
            ..Default::default()
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(!json.contains("profile"));
    }

    #[test]
    fn test_context_with_profile_name() {
        let ctx = Context {
            profile: ProfileRef::Name("lima".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"lima\""));
    }

    // -----------------------------------------------------------------------
    // Profile resolution tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_profile_name() {
        let mut config = SunbeamConfig::default();
        let mut profile = Profile::default();
        profile.rules.push(Rule {
            resource: "postgres".to_string(),
            namespace: None,
            kind: None,
            preset: None,
            shortcuts: HashMap::new(),
            containers: HashMap::new(),
            volumes: HashMap::new(),
            env: HashMap::new(),
        });
        config.profiles.insert("lima".to_string(), profile);

        let resolved = config.resolve_profile(&ProfileRef::Name("lima".to_string()));
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().rules[0].resource, "postgres");
    }

    #[test]
    fn test_resolve_profile_inline() {
        let config = SunbeamConfig::default();
        let profile = Profile {
            rules: vec![Rule {
                resource: "searxng".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            }],
            ..Default::default()
        };
        let resolved = config.resolve_profile(&ProfileRef::Inline(profile));
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().rules[0].resource, "searxng");
    }

    #[test]
    fn test_resolve_profile_none() {
        let config = SunbeamConfig::default();
        assert!(config.resolve_profile(&ProfileRef::None).is_none());
    }

    #[test]
    fn test_resolve_profile_missing_name() {
        let config = SunbeamConfig::default();
        assert!(
            config
                .resolve_profile(&ProfileRef::Name("nope".to_string()))
                .is_none()
        );
    }

    // -----------------------------------------------------------------------
    // Profile CRUD tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_add_rule_creates_profile() {
        let mut config = SunbeamConfig::default();
        config.add_rule(
            "lima",
            Rule {
                resource: "pingora".to_string(),
                namespace: Some("ingress".to_string()),
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );
        assert!(config.profiles.contains_key("lima"));
        assert_eq!(config.profiles["lima"].rules.len(), 1);
    }

    #[test]
    fn test_add_rule_replaces_existing() {
        let mut config = SunbeamConfig::default();
        config.add_rule(
            "lima",
            Rule {
                resource: "pingora".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );
        config.add_rule(
            "lima",
            Rule {
                resource: "pingora".to_string(),
                namespace: Some("ingress".to_string()),
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );
        assert_eq!(config.profiles["lima"].rules.len(), 1);
        assert_eq!(
            config.profiles["lima"].rules[0].namespace,
            Some("ingress".to_string())
        );
    }

    #[test]
    fn test_remove_rule() {
        let mut config = SunbeamConfig::default();
        config.add_rule(
            "lima",
            Rule {
                resource: "pingora".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );
        assert!(config.remove_rule("lima", "pingora"));
        assert!(config.profiles["lima"].rules.is_empty());
        assert!(!config.remove_rule("lima", "pingora"));
    }

    #[test]
    fn test_remove_rule_missing_profile() {
        let mut config = SunbeamConfig::default();
        assert!(!config.remove_rule("lima", "pingora"));
    }

    #[test]
    fn test_add_preset() {
        let mut config = SunbeamConfig::default();
        let mut preset = Preset::default();
        preset
            .values
            .insert("instances".to_string(), serde_json::json!(1));
        config.add_preset("lima", "tiny", preset);
        assert_eq!(
            config.profiles["lima"].presets["tiny"].values["instances"],
            1
        );
    }

    #[test]
    fn test_set_preset_merges() {
        let mut config = SunbeamConfig::default();
        let mut preset = Preset::default();
        preset
            .values
            .insert("instances".to_string(), serde_json::json!(1));
        config.add_preset("lima", "tiny", preset);

        let mut updates = HashMap::new();
        updates.insert("memory".to_string(), serde_json::json!("512Mi"));
        config.set_preset("lima", "tiny", updates);

        assert_eq!(
            config.profiles["lima"].presets["tiny"].values["instances"],
            1
        );
        assert_eq!(
            config.profiles["lima"].presets["tiny"].values["memory"],
            "512Mi"
        );
    }

    #[test]
    fn test_remove_preset() {
        let mut config = SunbeamConfig::default();
        config.add_preset("lima", "tiny", Preset::default());
        assert!(config.remove_preset("lima", "tiny"));
        assert!(!config.remove_preset("lima", "tiny"));
    }

    #[test]
    fn test_copy_profile() {
        let mut config = SunbeamConfig::default();
        config.add_rule(
            "lima",
            Rule {
                resource: "pingora".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );
        assert!(config.copy_profile("lima", "lima-big"));
        assert!(config.profiles.contains_key("lima-big"));
        assert_eq!(config.profiles["lima-big"].rules.len(), 1);
        // Mutate copy without affecting original
        config.add_rule(
            "lima-big",
            Rule {
                resource: "searxng".to_string(),
                namespace: None,
                kind: None,
                preset: None,
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );
        assert_eq!(config.profiles["lima"].rules.len(), 1);
        assert_eq!(config.profiles["lima-big"].rules.len(), 2);
    }

    #[test]
    fn test_copy_profile_missing() {
        let mut config = SunbeamConfig::default();
        assert!(!config.copy_profile("nope", "dst"));
    }

    #[test]
    fn test_global_preset() {
        let mut config = SunbeamConfig::default();
        let mut preset = Preset::default();
        preset
            .values
            .insert("scale".to_string(), serde_json::json!(0));
        config.add_global_preset("off", preset);
        assert!(config.presets.contains_key("off"));
        assert!(config.remove_global_preset("off"));
        assert!(!config.presets.contains_key("off"));
    }

    #[test]
    fn test_profile_crud_roundtrip() {
        let mut config = SunbeamConfig::default();
        let mut preset = Preset::default();
        preset
            .values
            .insert("instances".to_string(), serde_json::json!(1));
        config.add_preset("lima", "tiny", preset);
        config.add_rule(
            "lima",
            Rule {
                resource: "postgres".to_string(),
                namespace: None,
                kind: None,
                preset: Some("tiny".to_string()),
                shortcuts: HashMap::new(),
                containers: HashMap::new(),
                volumes: HashMap::new(),
                env: HashMap::new(),
            },
        );

        let json = serde_json::to_string_pretty(&config).unwrap();
        let loaded: SunbeamConfig = serde_json::from_str(&json).unwrap();
        assert!(loaded.profiles.contains_key("lima"));
        assert_eq!(
            loaded.profiles["lima"].presets["tiny"].values["instances"],
            1
        );
        assert_eq!(loaded.profiles["lima"].rules[0].resource, "postgres");
    }

    #[test]
    fn test_config_with_profile_ref_roundtrip() {
        let mut config = SunbeamConfig::default();
        config.contexts.insert(
            "lima-local".to_string(),
            Context {
                profile: ProfileRef::Name("lima".to_string()),
                domain: "192.168.5.15.sslip.io".to_string(),
                ..Default::default()
            },
        );
        let json = serde_json::to_string_pretty(&config).unwrap();
        let loaded: SunbeamConfig = serde_json::from_str(&json).unwrap();
        let ctx = loaded.contexts.get("lima-local").unwrap();
        assert!(matches!(&ctx.profile, ProfileRef::Name(n) if n == "lima"));
    }
}
