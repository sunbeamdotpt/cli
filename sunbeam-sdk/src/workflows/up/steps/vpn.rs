//! VPN steps: mint Headscale pre-auth keys for the subnet router and
//! the current user, idempotently.
//!
//! This step runs after headscale is deployed and its ACL ConfigMap is
//! loaded. It attaches into the headscale DaemonSet pod on the local
//! node and runs `headscale` CLI commands against the unix socket at
//! /var/run/headscale/headscale.sock (no API key required from inside
//! the pod). Two keys are produced:
//!
//!   - `tag:router` - written to the `subnet-router-authkey` Secret in
//!     the `vpn` namespace. Consumed by the subnet-router Deployment.
//!   - `tag:user` - written to `~/.sunbeam/config.json` under the
//!     current context's `vpn-auth-key` field. Consumed by `sunbeam
//!     connect` on the operator's laptop.
//!
//! Both writes are skip-if-exists: `sunbeam up` re-runs this step on
//! every invocation, and we want it to be a no-op after the first
//! successful run.
//!
//! If the headscale DaemonSet isn't running we FAIL LOUDLY rather than
//! silently skipping - if the operator expected VPN to come up and it
//! didn't, they should see the error at `sunbeam up` time, not hours
//! later when `sunbeam connect` mysteriously has no auth key.

use k8s_openapi::api::core::v1::Secret;
use kube::api::Api;
use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::{info, trace};

use crate::workflows::data::UpData;

const HEADSCALE_NS: &str = "vpn";
const HEADSCALE_LABEL: &str = "app.kubernetes.io/name=headscale";
const HEADSCALE_USER: &str = "sienna";
const ROUTER_TAG: &str = "tag:router";
const USER_TAG: &str = "tag:user";
const ROUTER_SECRET: &str = "subnet-router-authkey";
const ROUTER_SECRET_KEY: &str = "authkey";
/// Pre-auth key lifetime. Long enough to avoid routine rotation churn
/// on the subnet router (which otherwise needs to re-register whenever
/// its state Secret cycles), short enough that a leaked key expires.
const KEY_EXPIRATION: &str = "8760h";

/// Mint Headscale pre-auth keys for the subnet router and for the
/// current user. Idempotent: re-running is a no-op when both sinks
/// already have a usable key.
pub struct MintVpnPreAuthKeys {
    logger: crate::logger::Logger,
}

impl Default for MintVpnPreAuthKeys {
    fn default() -> Self {
        Self {
            logger: crate::logger::Logger::new(crate::logger::TracingSink),
        }
    }
}

#[async_trait::async_trait]
impl StepBody for MintVpnPreAuthKeys {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let logger = &self.logger;
        let _data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        let skip_namespaces: Vec<String> = ctx
            .workflow
            .data
            .get("skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if skip_namespaces.contains(&HEADSCALE_NS.to_string()) {
            info!(logger, "Skipping VPN pre-auth keys (profile skip list)");
            return Ok(ExecutionResult::next());
        }

        info!(logger, "VPN pre-auth keys...");

        // Both sinks already populated, nothing to do.
        let router_secret_ok = router_secret_has_key()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(format!("probe router secret: {e}")))?;
        let user_key_ok = user_config_has_key();

        if router_secret_ok && user_key_ok {
            info!(logger, "VPN pre-auth keys already present - skipping.");
            return Ok(ExecutionResult::next());
        }

        // Locate headscale pod (must be present; fail loudly otherwise).
        let pod = k::find_pod_by_label(HEADSCALE_NS, HEADSCALE_LABEL)
            .await
            .ok_or_else(|| {
                wfe_core::WfeError::StepExecution(
                    "headscale DaemonSet not running - cannot mint pre-auth keys".into(),
                )
            })?;

        // Ensure the headscale user is present before we mint keys
        // against its tags.
        ensure_headscale_user(&pod, HEADSCALE_USER)
            .await
            .map_err(|e| {
                wfe_core::WfeError::StepExecution(format!("ensure headscale user: {e}"))
            })?;

        // Router key -> Secret.
        if !router_secret_ok {
            let key = mint_preauth_key(&pod, HEADSCALE_USER, ROUTER_TAG, KEY_EXPIRATION)
                .await
                .map_err(|e| wfe_core::WfeError::StepExecution(format!("mint router key: {e}")))?;
            write_router_secret(&key).await.map_err(|e| {
                wfe_core::WfeError::StepExecution(format!("write router secret: {e}"))
            })?;
            info!(logger, "Minted router pre-auth key -> Secret vpn/subnet-router-authkey");
        } else {
            info!(logger, "Router Secret already present - skipping mint.");
        }

        // User key -> ~/.sunbeam/config.json.
        if !user_key_ok {
            let key = mint_preauth_key(&pod, HEADSCALE_USER, USER_TAG, KEY_EXPIRATION)
                .await
                .map_err(|e| wfe_core::WfeError::StepExecution(format!("mint user key: {e}")))?;
            write_user_config_key(&key).map_err(|e| {
                wfe_core::WfeError::StepExecution(format!(
                    "Failed to persist user key to config: {e}"
                ))
            })?;
            info!(logger, "Minted user pre-auth key -> ~/.sunbeam/config.json (vpn-auth-key)");
        } else {
            info!(logger, "User vpn-auth-key already present - skipping mint.");
        }

        Ok(ExecutionResult::next())
    }
}

// -- helpers ----------------------------------------------------------------

/// Return true if the `subnet-router-authkey` Secret already exists
/// and has a non-empty `authkey` field.
async fn router_secret_has_key() -> crate::error::Result<bool> {
    let client = k::get_client().await?;
    let api: Api<Secret> = Api::namespaced(client.clone(), HEADSCALE_NS);
    let secret = match api.get_opt(ROUTER_SECRET).await? {
        Some(s) => s,
        None => return Ok(false),
    };
    let has = secret
        .data
        .as_ref()
        .and_then(|d| d.get(ROUTER_SECRET_KEY))
        .map(|v| !v.0.is_empty())
        .unwrap_or(false);
    Ok(has)
}

/// Return true if the current context's `vpn-auth-key` is already set.
fn user_config_has_key() -> bool {
    let cfg = crate::config::load_config();
    let name = if cfg.current_context.is_empty() {
        "default".to_string()
    } else {
        cfg.current_context.clone()
    };
    cfg.contexts
        .get(&name)
        .map(|c| !c.vpn_auth_key.is_empty())
        .unwrap_or(false)
}

/// Ensure the named headscale user is present. Runs `headscale users
/// list` and creates the user if absent.
async fn ensure_headscale_user(pod: &str, user: &str) -> crate::error::Result<()> {
    let (_rc, stdout) = k::kube_exec(
        HEADSCALE_NS,
        pod,
        &["headscale", "users", "list"],
        Some("headscale"),
    )
    .await?;

    if user_present_in_list(&stdout, user) {
        return Ok(());
    }

    let (rc, out) = k::kube_exec(
        HEADSCALE_NS,
        pod,
        &["headscale", "users", "create", user],
        Some("headscale"),
    )
    .await?;

    if rc != 0 && !out.contains("already exists") {
        return Err(crate::error::SunbeamError::Other(format!(
            "headscale users create {user} rc={rc}: {out}"
        )));
    }
    Ok(())
}

/// Parse `headscale users list` stdout and return true if the named
/// user appears in the table.
pub(crate) fn user_present_in_list(stdout: &str, user: &str) -> bool {
    for line in stdout.lines() {
        // Tabular output typically uses `|` separators.
        if line.split('|').any(|f| f.trim() == user) {
            return true;
        }
        // Whitespace-separated fallback.
        if line.split_whitespace().any(|f| f == user) {
            return true;
        }
    }
    false
}

/// Mint a pre-auth key for the given user + tag and return just the
/// raw key string. Headscale 0.23 writes JSON output to stdout as a
/// single object when `--output json` is passed.
async fn mint_preauth_key(
    pod: &str,
    user: &str,
    tag: &str,
    expiration: &str,
) -> crate::error::Result<String> {
    let (rc, stdout) = k::kube_exec(
        HEADSCALE_NS,
        pod,
        &[
            "headscale",
            "preauthkeys",
            "create",
            "--user",
            user,
            "--tags",
            tag,
            "--expiration",
            expiration,
            "--output",
            "json",
        ],
        Some("headscale"),
    )
    .await?;

    if rc != 0 {
        return Err(crate::error::SunbeamError::Other(format!(
            "headscale preauthkeys create rc={rc}: {stdout}"
        )));
    }

    parse_preauth_key(&stdout).ok_or_else(|| {
        crate::error::SunbeamError::Other(format!(
            "could not parse pre-auth key from headscale output: {stdout}"
        ))
    })
}

/// Parse a pre-auth key out of headscale's CLI output. Handles both
/// JSON (from `--output json`) and the plain-text fallback where the
/// key is printed on a single line.
pub(crate) fn parse_preauth_key(stdout: &str) -> Option<String> {
    // JSON form: { "key": "...", "user": ..., "tags": [...] } or
    // nested under "preAuthKey". Try to extract "key" field first.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        if let Some(k) = v.get("key").and_then(|k| k.as_str())
            && !k.is_empty()
        {
            return Some(k.to_string());
        }
        if let Some(k) = v
            .get("preAuthKey")
            .and_then(|p| p.get("key"))
            .and_then(|k| k.as_str())
            && !k.is_empty()
        {
            return Some(k.to_string());
        }
    }

    // Plain-text fallback: pick the longest alphanumeric token that
    // looks like a pre-auth key (>= 32 chars, base32/base64ish alphabet).
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        for token in line.split_whitespace().rev() {
            if token.len() >= 32
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Write the router pre-auth key to the `subnet-router-authkey` Secret.
async fn write_router_secret(key: &str) -> crate::error::Result<()> {
    let mut data = std::collections::HashMap::new();
    data.insert(ROUTER_SECRET_KEY.to_string(), key.to_string());
    k::create_secret(HEADSCALE_NS, ROUTER_SECRET, data).await
}

/// Persist the user pre-auth key into the current context's
/// `vpn-auth-key` field in ~/.sunbeam/config.json. No-op if the field
/// is already set.
fn write_user_config_key(key: &str) -> crate::error::Result<()> {
    let mut cfg = crate::config::load_config();
    let name = if cfg.current_context.is_empty() {
        "default".to_string()
    } else {
        cfg.current_context.clone()
    };
    let entry = cfg.contexts.entry(name).or_default();
    if !entry.vpn_auth_key.is_empty() {
        return Ok(());
    }
    entry.vpn_auth_key = key.to_string();
    crate::config::save_config(&cfg)
}

// -- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_vpn_preauth_keys_is_default() {
        let _ = MintVpnPreAuthKeys {
            logger: crate::logger::Logger::new(crate::logger::NoopSink),
        };
    }

    #[test]
    fn parse_json_flat_key() {
        let out = r#"{"key":"abcdefghijklmnopqrstuvwxyz012345","user":"sienna"}"#;
        let k = parse_preauth_key(out).unwrap();
        assert_eq!(k, "abcdefghijklmnopqrstuvwxyz012345");
    }

    #[test]
    fn parse_json_nested_key() {
        let out = r#"{"preAuthKey":{"key":"NESTED_KEY_abcdefghijklmnopqrstuvwxyz"}}"#;
        let k = parse_preauth_key(out).unwrap();
        assert_eq!(k, "NESTED_KEY_abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn parse_plaintext_fallback() {
        let out = "ID | Key                                | Reusable | Ephemeral\n\
                   1  | ABCDEFabcdef0123456789012345678901  | false    | false\n";
        let k = parse_preauth_key(out).unwrap();
        assert_eq!(k, "ABCDEFabcdef0123456789012345678901");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_preauth_key("").is_none());
        assert!(parse_preauth_key("short").is_none());
        assert!(parse_preauth_key("{}").is_none());
    }

    #[test]
    fn parse_rejects_empty_json_key() {
        assert!(parse_preauth_key(r#"{"key":""}"#).is_none());
    }

    #[test]
    fn parse_handles_key_with_dot() {
        let out = r#"{"key":"nodekey.abcdef0123456789abcdef01234567"}"#;
        let k = parse_preauth_key(out).unwrap();
        assert_eq!(k, "nodekey.abcdef0123456789abcdef01234567");
    }

    #[test]
    fn parse_handles_multiline_plaintext() {
        let out = "Pre-auth key created:\n\n\
                   key_0123456789abcdef0123456789abcdef\n";
        let k = parse_preauth_key(out).unwrap();
        assert_eq!(k, "key_0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn user_present_pipe_separated() {
        let out = "ID | Name   | Created\n\
                   1  | sienna | 2026-04-11\n";
        assert!(user_present_in_list(out, "sienna"));
        assert!(!user_present_in_list(out, "bob"));
    }

    #[test]
    fn user_present_whitespace_separated() {
        let out = "NAME     CREATED\n\
                   sienna   2026-04-11\n";
        assert!(user_present_in_list(out, "sienna"));
    }

    #[test]
    fn user_present_empty_output() {
        assert!(!user_present_in_list("", "sienna"));
    }

    #[test]
    fn user_config_no_key_when_empty_config() {
        // Just checks the function doesn't panic. The actual file state
        // depends on the environment.
        let _ = user_config_has_key();
    }
}
