//! ApplyManifest — atomic step that applies kustomize manifests for a single namespace.
//!
//! Reads `step_config.namespace` and resolves domain/email from workflow data.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::output::step;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Apply kustomize manifests for one namespace.
///
/// **step_config:** `{"namespace": "ory"}`
///
/// Reads `__ctx` (domain, acme_email) and `domain` from workflow data.
#[derive(Default)]
pub struct ApplyManifest;

#[async_trait::async_trait]
impl StepBody for ApplyManifest {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("ApplyManifest: missing step_config"))?;
        let namespace = config
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("ApplyManifest: missing namespace in step_config"))?;

        let data = &ctx.workflow.data;

        let domain = data.get("domain").and_then(|v| v.as_str()).unwrap_or("");
        let domain = if domain.is_empty() {
            data.get("__ctx")
                .and_then(|c| c.get("domain"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        } else {
            domain
        };

        let email = data
            .get("__ctx")
            .and_then(|c| c.get("acme_email"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Auto-skip Scaleway DNS webhook resources on local dev domains
        // (sslip.io, nip.io, etc.) since they require external DNS creds.
        let mut skip_patterns: Vec<String> = config
            .get("skip_patterns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if domain.ends_with("sslip.io") || domain.ends_with("nip.io") {
            for pat in &[
                "scaleway-certmanager-webhook",
                "letsencrypt-staging",
                "letsencrypt-production",
                "pingora-tls",
            ] {
                if !skip_patterns.contains(&pat.to_string()) {
                    skip_patterns.push(pat.to_string());
                }
            }
        }

        step(&format!("Applying {namespace}..."));

        let overrides = data
            .get("manifest_overrides")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        crate::manifests::cmd_apply(domain, email, namespace, &skip_patterns, overrides.as_ref())
            .await
            .map_err(|e| step_err(e.to_string()))?;

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_manifest_is_default() {
        let _ = ApplyManifest;
    }

    #[test]
    fn missing_step_config_is_descriptive() {
        let err = step_err("ApplyManifest: missing step_config");
        let msg = err.to_string();
        assert!(
            msg.contains("ApplyManifest"),
            "error should name the step: {msg}"
        );
        assert!(
            msg.contains("step_config"),
            "error should mention step_config: {msg}"
        );
    }
}
