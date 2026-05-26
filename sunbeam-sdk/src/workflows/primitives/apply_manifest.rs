//! ApplyManifest — atomic step that applies kustomize manifests for a single namespace.
//!
//! Reads `step_config.namespace` and manifest overrides from workflow data.
//! Domain and email are read from the globally-resolved active context.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};



fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Build the skip-patterns list from step config + domain heuristic.
fn build_skip_patterns(step_config: &serde_json::Value, domain: &str) -> Vec<String> {
    let mut skip_patterns: Vec<String> = step_config
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
    skip_patterns
}

/// Apply kustomize manifests for one namespace.
///
/// **step_config:** `{"namespace": "ory"}`
///
/// Domain/email are taken from `config::active_context()` (resolved once at
/// the CLI boundary). Overrides are read from `workflow.data.manifest_overrides`.
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

        let domain = &crate::config::active_context().domain;
        let skip_patterns = build_skip_patterns(config, domain);

        tracing::info!("Applying {namespace}...");

        let overrides = data
            .get("manifest_overrides")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let opts = crate::manifests::ApplyOptions {
            namespace: namespace.to_string(),
            skip_patterns,
            overrides,
            ..Default::default()
        };
        crate::manifests::apply_manifests(&opts)
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

    #[test]
    fn build_skip_patterns_empty_config_no_domain() {
        let config = serde_json::json!({"namespace": "ory"});
        let patterns = build_skip_patterns(&config, "sunbeam.pt");
        assert!(patterns.is_empty());
    }

    #[test]
    fn build_skip_patterns_sslip_io_adds_defaults() {
        let config = serde_json::json!({"namespace": "ory"});
        let patterns = build_skip_patterns(&config, "192.168.1.1.sslip.io");
        assert!(patterns.contains(&"scaleway-certmanager-webhook".to_string()));
        assert!(patterns.contains(&"letsencrypt-staging".to_string()));
        assert!(patterns.contains(&"letsencrypt-production".to_string()));
        assert!(patterns.contains(&"pingora-tls".to_string()));
    }

    #[test]
    fn build_skip_patterns_merges_config_with_defaults() {
        let config = serde_json::json!({
            "namespace": "ory",
            "skip_patterns": ["custom-webhook"]
        });
        let patterns = build_skip_patterns(&config, "10.0.0.1.nip.io");
        assert!(patterns.contains(&"custom-webhook".to_string()));
        assert!(patterns.contains(&"scaleway-certmanager-webhook".to_string()));
    }

    #[test]
    fn build_skip_patterns_does_not_duplicate_defaults() {
        let config = serde_json::json!({
            "namespace": "ory",
            "skip_patterns": ["pingora-tls"]
        });
        let patterns = build_skip_patterns(&config, "10.0.0.1.nip.io");
        let pingora_count = patterns.iter().filter(|p| *p == "pingora-tls").count();
        assert_eq!(pingora_count, 1);
    }
}
