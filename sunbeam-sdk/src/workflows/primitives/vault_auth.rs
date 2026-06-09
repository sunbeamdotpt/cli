//! Vault auth primitives — atomic steps for OpenBao auth configuration.
//!
//! Each step creates its own port-forward to OpenBao, performs one operation, and drops it.
//! Reads `ob_pod`, `root_token`, `skip_seed` from workflow data.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::openbao::BaoClient;

use crate::secrets;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

fn should_skip(data: &serde_json::Value) -> bool {
    data.get("skip_seed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn get_str(data: &serde_json::Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

async fn connect_bao(
    data: &serde_json::Value,
) -> Result<(BaoClient, secrets::PortForwardGuard), wfe_core::WfeError> {
    let ob_pod = get_str(data, "ob_pod").ok_or_else(|| step_err("vault auth: missing ob_pod"))?;
    let root_token =
        get_str(data, "root_token").ok_or_else(|| step_err("vault auth: missing root_token"))?;
    let pf = secrets::port_forward("openbao", &ob_pod, 8200)
        .await
        .map_err(|e| step_err(e.to_string()))?;
    let bao = BaoClient::with_token(&format!("http://127.0.0.1:{}", pf.local_port), &root_token);
    Ok((bao, pf))
}

// ── EnableVaultAuth ─────────────────────────────────────────────────────────

/// Enable an auth method at a mount path.
///
/// **step_config:** `{"mount": "kubernetes", "type": "kubernetes"}`
#[derive(Default)]
pub struct EnableVaultAuth;

#[async_trait::async_trait]
impl StepBody for EnableVaultAuth {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;
        if should_skip(data) {
            tracing::info!(msg = "Skipping vault auth enable (skip_seed).");
            return Ok(ExecutionResult::next());
        }
        if get_str(data, "ob_pod").is_none() || get_str(data, "root_token").is_none() {
            tracing::info!(msg = "Skipping vault auth enable (missing ob_pod or root_token).");
            return Ok(ExecutionResult::next());
        }

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("EnableVaultAuth: missing step_config"))?;
        let mount = config
            .get("mount")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("EnableVaultAuth: missing mount"))?;
        let auth_type = config.get("type").and_then(|v| v.as_str()).unwrap_or(mount);

        let (bao, _pf) = connect_bao(data).await?;
        let _ = bao.auth_enable(mount, auth_type).await;
        tracing::info!("Vault auth enabled: {mount}");
        Ok(ExecutionResult::next())
    }
}

// ── WriteVaultAuthConfig ────────────────────────────────────────────────────

/// Write auth method configuration.
///
/// **step_config:** `{"mount": "kubernetes", "config": {"kubernetes_host": "..."}}`
#[derive(Default)]
pub struct WriteVaultAuthConfig;

#[async_trait::async_trait]
impl StepBody for WriteVaultAuthConfig {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;
        if should_skip(data) {
            tracing::info!(msg = "Skipping vault auth config (skip_seed).");
            return Ok(ExecutionResult::next());
        }
        if get_str(data, "ob_pod").is_none() || get_str(data, "root_token").is_none() {
            tracing::info!(msg = "Skipping vault auth config (missing ob_pod or root_token).");
            return Ok(ExecutionResult::next());
        }

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("WriteVaultAuthConfig: missing step_config"))?;
        let mount = config
            .get("mount")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WriteVaultAuthConfig: missing mount"))?;
        let auth_config = config
            .get("config")
            .ok_or_else(|| step_err("WriteVaultAuthConfig: missing config"))?;

        let (bao, _pf) = connect_bao(data).await?;
        bao.write(&format!("auth/{mount}/config"), auth_config)
            .await
            .map_err(|e| step_err(format!("WriteVaultAuthConfig({mount}): {e}")))?;
        tracing::info!("Vault auth config: {mount}");
        Ok(ExecutionResult::next())
    }
}

// ── WriteVaultPolicy ────────────────────────────────────────────────────────

/// Write a Vault/OpenBao policy.
///
/// **step_config:** `{"name": "vso-reader", "hcl": "path \"secret/data/*\" { ... }"}`
#[derive(Default)]
pub struct WriteVaultPolicy;

#[async_trait::async_trait]
impl StepBody for WriteVaultPolicy {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;
        if should_skip(data) {
            tracing::info!(msg = "Skipping vault policy write (skip_seed).");
            return Ok(ExecutionResult::next());
        }
        if get_str(data, "ob_pod").is_none() || get_str(data, "root_token").is_none() {
            tracing::info!(msg = "Skipping vault policy write (missing ob_pod or root_token).");
            return Ok(ExecutionResult::next());
        }

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("WriteVaultPolicy: missing step_config"))?;
        let name = config
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WriteVaultPolicy: missing name"))?;
        let hcl = config
            .get("hcl")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WriteVaultPolicy: missing hcl"))?;

        let (bao, _pf) = connect_bao(data).await?;
        bao.write_policy(name, hcl)
            .await
            .map_err(|e| step_err(format!("WriteVaultPolicy({name}): {e}")))?;
        tracing::info!("Vault policy: {name}");
        Ok(ExecutionResult::next())
    }
}

// ── WriteVaultRole ──────────────────────────────────────────────────────────

/// Write an auth method role.
///
/// **step_config:** `{"mount": "kubernetes", "role": "vso", "config": {...}}`
#[derive(Default)]
pub struct WriteVaultRole;

#[async_trait::async_trait]
impl StepBody for WriteVaultRole {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;
        if should_skip(data) {
            tracing::info!(msg = "Skipping vault role write (skip_seed).");
            return Ok(ExecutionResult::next());
        }
        if get_str(data, "ob_pod").is_none() || get_str(data, "root_token").is_none() {
            tracing::info!(msg = "Skipping vault role write (missing ob_pod or root_token).");
            return Ok(ExecutionResult::next());
        }

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("WriteVaultRole: missing step_config"))?;
        let mount = config
            .get("mount")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WriteVaultRole: missing mount"))?;
        let role = config
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WriteVaultRole: missing role"))?;
        let role_config = config
            .get("config")
            .ok_or_else(|| step_err("WriteVaultRole: missing config"))?;

        let (bao, _pf) = connect_bao(data).await?;
        bao.write(&format!("auth/{mount}/role/{role}"), role_config)
            .await
            .map_err(|e| step_err(format!("WriteVaultRole({mount}/{role}): {e}")))?;
        tracing::info!("Vault role: {mount}/{role}");
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_vault_auth_steps_are_default() {
        let _ = EnableVaultAuth;
        let _ = WriteVaultAuthConfig;
        let _ = WriteVaultPolicy;
        let _ = WriteVaultRole;
    }

    #[test]
    fn should_skip_true() {
        assert!(should_skip(&serde_json::json!({"skip_seed": true})));
    }

    #[test]
    fn should_skip_false() {
        assert!(!should_skip(&serde_json::json!({"skip_seed": false})));
        assert!(!should_skip(&serde_json::json!({})));
    }
}
