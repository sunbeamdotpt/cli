//! ApplyManifest — atomic step that applies kustomize manifests for a single namespace.
//!
//! Reads `step_config.namespace` and manifest overrides from workflow data.
//! Domain and email are read from the globally-resolved active context.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

/// Global mutex that serializes ApplyManifest steps on Lima VMs.
/// Single-node k3s cannot handle the pod-startup storm from many
/// namespaces applied in parallel, even with a kube_apply semaphore.
static LIMA_APPLY_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

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

/// True if the error looks like a transient connection failure to the
/// Kubernetes API server (e.g. during k3s overload on a single-node Lima VM).
fn is_transient_connection_err(e: &crate::error::SunbeamError) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("connect")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("reset by peer")
        || msg.contains("timeout")
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

        // On Lima VMs, serialize all ApplyManifest steps to protect single-node
        // k3s from being overwhelmed by concurrent namespace applications.
        let use_lima = data
            .get("use_lima")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let lima_skip: Vec<String> = data
            .get("lima_skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if lima_skip.contains(&namespace.to_string()) {
            tracing::info!("Skipping {namespace} namespace apply (Lima VM)");
            return Ok(ExecutionResult::next());
        }

        let mut overrides: crate::manifest_params::Overrides = data
            .get("manifest_overrides")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // On Lima dev VMs, disk is limited — shrink production-sized PVCs.
        // Also shrink postgres resources so the initdb job can schedule on a
        // 10 GiB node alongside the data-namespace pods.
        if use_lima && namespace == "data" {
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "cluster/data/postgres".into(),
                field_path: "spec/storage/size".into(),
                value: "10Gi".into(),
            });
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "persistentvolumeclaim/data/opensearch-data".into(),
                field_path: "spec/resources/requests/storage".into(),
                value: "5Gi".into(),
            });
            // Undo production-overlay sizing (4 Gi req / 8 Gi lim) so initdb
            // and the running instance fit on a single-node Lima VM.
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "cluster/data/postgres".into(),
                field_path: "spec/resources".into(),
                value: r#"{"requests":{"memory":"512Mi","cpu":"250m"},"limits":{"memory":"1Gi"}}"#
                    .into(),
            });
            // Restore base postgresql parameters (undo production overlay).
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "cluster/data/postgres".into(),
                field_path: "spec/postgresql/parameters".into(),
                value: r#"{"max_connections":"100","shared_buffers":"128MB","work_mem":"4MB","effective_cache_size":"256MB","maintenance_work_mem":"64MB"}"#
                    .into(),
            });
            // Scale searxng to zero — it crashes anyway and wastes 1 Gi of
            // scheduling headroom on a tiny node.
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "deployment/data/searxng".into(),
                field_path: "spec/replicas".into(),
                value: "0".into(),
            });
            // Shrink OpenSearch to 512 Mi req / 1 Gi lim so it fits alongside
            // postgres on a 10 Gi node. JVM heap is capped at 512 Mi.
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "deployment/data/opensearch".into(),
                field_path: "spec/template/spec/containers/0/resources".into(),
                value: r#"{"requests":{"memory":"512Mi","cpu":"100m"},"limits":{"memory":"1Gi"}}"#
                    .into(),
            });
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "deployment/data/opensearch".into(),
                field_path: "spec/template/spec/containers/0/env".into(),
                value: r#"[{"name":"discovery.type","value":"single-node"},{"name":"OPENSEARCH_JAVA_OPTS","value":"-Xms256m -Xmx512m"},{"name":"DISABLE_SECURITY_PLUGIN","value":"true"},{"name":"plugins.ml_commons.only_run_on_ml_node","value":"false"},{"name":"plugins.ml_commons.native_memory_threshold","value":"90"},{"name":"plugins.ml_commons.model_access_control_enabled","value":"false"},{"name":"plugins.ml_commons.allow_registering_model_via_url","value":"true"}]"#
                    .into(),
            });
        }

        // On Lima VMs, the unified overlay adds hostPort 22 (SSH), 80, and 443
        // to the pingora deployment. hostPort 22 conflicts with the VM's own
        // SSH daemon and breaks Lima's guest agent forwarding. Strip all
        // hostPorts so pingora uses container ports only.
        if use_lima && namespace == "ingress" {
            overrides.items.push(crate::manifest_params::Override::Set {
                resource: "deployment/ingress/pingora".into(),
                field_path: "spec/template/spec/containers/0/ports".into(),
                value: r#"[{"name":"http","containerPort":80,"protocol":"TCP"},{"name":"https","containerPort":443,"protocol":"TCP"},{"name":"ssh","containerPort":22,"protocol":"TCP"}]"#.into(),
            });
        }

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

        let _lima_guard = if use_lima {
            let lock = LIMA_APPLY_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
            tracing::info!("Applying {namespace} (Lima serialized)...");
            let guard = lock.lock().await;
            // Give single-node k3s a moment to breathe between namespace
            // applications — pod startup storms from previous namespaces can
            // make the API server temporarily unresponsive.
            let base_delay = config
                .get("lima_delay_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);
            // In serial mode, double the breathing room so pods from the
            // previous namespace have time to start before we hammer the
            // scheduler with the next namespace.
            let delay = if serial_mode { base_delay * 3 } else { base_delay };
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            Some(guard)
        } else {
            // Stagger parallel steps to avoid thundering-herd against k3s API.
            let stagger = if serial_mode {
                // In serial mode, add a generous fixed stagger regardless of
                // namespace name so we don't flood the API server.
                2000u64 + stagger_millis_for(namespace)
            } else {
                stagger_millis_for(namespace)
            };
            if stagger > 0 {
                tracing::info!("Applying {namespace} (staggering {stagger}ms)...");
                tokio::time::sleep(std::time::Duration::from_millis(stagger)).await;
            } else {
                tracing::info!("Applying {namespace}...");
            }
            None
        };

        // Retry on transient connection errors — single-node k3s can briefly
        // become unresponsive when many namespaces are applied in parallel.
        let mut last_err = None;
        for attempt in 1..=5 {
            match crate::manifests::apply_manifests(&opts).await {
                Ok(_) => {
                    if attempt > 1 {
                        tracing::info!("Applied {namespace} on attempt {attempt}");
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
                        tracing::warn!(
                            "Apply {namespace} attempt {attempt} failed (transient), retrying in {backoff}s..."
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
