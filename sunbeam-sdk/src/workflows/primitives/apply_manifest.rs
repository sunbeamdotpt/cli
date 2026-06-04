//! ApplyManifest — atomic step that applies kustomize manifests for a single namespace.
//!
//! Reads `step_config.namespace` and manifest overrides from workflow data.
//! Domain and email are read from the globally-resolved active context.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::logger::{Logger, TracingSink};
use crate::info;

/// Global mutex that serializes ApplyManifest steps when running in serial mode.
/// Single-node k3s cannot handle the pod-startup storm from many
/// namespaces applied in parallel, even with a kube_apply semaphore.
static SERIAL_APPLY_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

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

/// True if the error looks like a transient failure we can recover from by
/// retrying: connection issues (k3s overload) or 404s from API endpoints that
/// aren't registered yet (CRD registration race).
fn is_transient_connection_err(e: &crate::error::SunbeamError) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("connect")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("reset by peer")
        || msg.contains("timeout")
        || msg.contains("404")
        || msg.contains("not found")
}

/// Compute a small stagger delay (0–2 s) from a namespace name so that
/// parallel ApplyManifest steps don't all hammer the API server at once.
fn stagger_millis_for(name: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    (h.finish() % 5) * 500
}

/// Apply kustomize manifests for one namespace.
///
/// **step_config:** `{"namespace": "ory"}`
///
/// Domain/email are taken from `config::active_context()` (resolved once at
/// the CLI boundary). Overrides are read from `workflow.data.manifest_overrides`.
pub struct ApplyManifest {
    logger: Logger,
}

impl Default for ApplyManifest {
    fn default() -> Self {
        Self {
            logger: Logger::new(TracingSink),
        }
    }
}

#[async_trait::async_trait]
impl StepBody for ApplyManifest {
    #[tracing::instrument(skip(self, ctx), fields(step = "ApplyManifest"))]
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

        // Workflow steps like EnsureCilium may discover a live domain (e.g.
        // Lima VM IP) that differs from the statically-configured active
        // context. Prefer the live domain so manifests and images agree.
        let live_domain = data
            .get("domain")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let domain = live_domain
            .unwrap_or(&crate::config::active_context().domain);
        let skip_patterns = build_skip_patterns(config, domain);

        let skip_namespaces: Vec<String> = data
            .get("skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if skip_namespaces.contains(&namespace.to_string()) {
            self.logger
                .info(&format!("Skipping {namespace} namespace apply (profile skip list)"), &[]);
            return Ok(ExecutionResult::next());
        }

        let overrides: crate::manifest_params::Overrides = data
            .get("manifest_overrides")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let opts = crate::manifests::ApplyOptions {
            namespace: namespace.to_string(),
            skip_patterns,
            overrides: Some(overrides),
            domain: live_domain.map(String::from),
            ..Default::default()
        };
        let serial_mode = data
            .get("serial_mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let _serial_guard = if serial_mode {
            let lock = SERIAL_APPLY_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
            self.logger
                .info(&format!("Applying {namespace} (serial mode)..."), &[]);
            let guard = lock.lock().await;
            // Give single-node k3s a moment to breathe between namespace
            // applications — pod startup storms from previous namespaces can
            // make the API server temporarily unresponsive.
            let base_delay = config
                .get("serial_delay_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);
            let delay = base_delay * 3;
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            Some(guard)
        } else {
            // Stagger parallel steps to avoid thundering-herd against k3s API.
            let stagger = stagger_millis_for(namespace);
            if stagger > 0 {
                self.logger
                    .info(&format!("Applying {namespace} (staggering {stagger}ms)..."), &[]);
                tokio::time::sleep(std::time::Duration::from_millis(stagger)).await;
            } else {
                self.logger
                    .info(&format!("Applying {namespace}..."), &[]);
            }
            None
        };

        // Retry on transient connection errors — single-node k3s can briefly
        // become unresponsive when many namespaces are applied in parallel.
        let mut last_err = None;
        for attempt in 1..=5 {
            match crate::manifests::apply_manifests(&self.logger, &opts).await {
                Ok(_) => {
                    if attempt > 1 {
                        self.logger
                            .info(&format!("Applied {namespace} on attempt {attempt}"), &[]);
                    } else {
                        info!(self.logger, "Manifests applied.", namespace = namespace);
                    }
                    return Ok(ExecutionResult::next());
                }
                Err(e) => {
                    last_err = Some(e);
                    if let Some(ref err) = last_err {
                        if !is_transient_connection_err(err) || attempt == 5 {
                            break;
                        }
                        let backoff = 1u64 << attempt; // 2, 4, 8, 16 s
                        self.logger.error(
                            &format!(
                                "Apply {namespace} attempt {attempt} failed (transient), retrying in {backoff}s..."
                            ),
                            &[],
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    }
                }
            }
        }

        Err(step_err(
            last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| format!("Failed to apply {namespace}")),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_manifest_is_default() {
        let _ = ApplyManifest::default();
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
