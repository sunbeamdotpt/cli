//! OpenBao initialization steps: find pod, wait for Running, init/unseal, enable KV.
//!
//! These steps are data-struct-agnostic — they read/write individual JSON fields
//! rather than deserializing a full typed struct. This makes them reusable across
//! the `seed`, `up`, and `verify` workflows.

use std::collections::HashMap;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::{ok, warn};
use crate::secrets;
use crate::workflows::StepContext;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

// ── FindOpenBaoPod ──────────────────────────────────────────────────────────

/// Find the OpenBao server pod by label selector.
/// Reads `__ctx` to set kube context. Sets `ob_pod` or `skip_seed=true`.
#[derive(Default)]
pub struct FindOpenBaoPod;

#[async_trait::async_trait]
impl StepBody for FindOpenBaoPod {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        let step_ctx: StepContext = serde_json::from_value(
            ctx.workflow.data.get("__ctx").cloned().unwrap_or_default()
        ).map_err(|e| step_err(e.to_string()))?;

        k::set_context(&step_ctx.kube_context, &step_ctx.ssh_host);

        let client = k::get_client().await.map_err(|e| step_err(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "data");
        let lp = kube::api::ListParams::default()
            .labels("app.kubernetes.io/name=openbao,component=server");
        let pod_list = pods.list(&lp).await.map_err(|e| step_err(e.to_string()))?;

        let ob_pod = pod_list
            .items
            .first()
            .and_then(|p| p.metadata.name.as_deref());

        let mut result = ExecutionResult::next();
        match ob_pod {
            Some(name) => {
                ok(&format!("OpenBao ({name})..."));
                result.output_data = Some(serde_json::json!({ "ob_pod": name }));
            }
            None => {
                ok("OpenBao pod not found -- skipping.");
                result.output_data = Some(serde_json::json!({ "skip_seed": true }));
            }
        }

        Ok(result)
    }
}

// ── WaitPodRunning ─────────────────────────────────────────────────────────

/// Wait for the OpenBao pod to reach Running state (up to 5 min).
/// Reads `ob_pod`, `skip_seed`. No-op if skip_seed or ob_pod is absent.
#[derive(Default)]
pub struct WaitPodRunning;

#[async_trait::async_trait]
impl StepBody for WaitPodRunning {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        if ctx.workflow.data.get("skip_seed").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match ctx.workflow.data.get("ob_pod").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(ExecutionResult::next()),
        };

        // Ensure openbao-keys secret exists (even as placeholder) so the pod
        // can mount it. InitOrUnsealOpenBao will overwrite with real values.
        if k::kube_get_secret_field("data", "openbao-keys", "key").await.is_err() {
            let placeholder = std::collections::HashMap::from([
                ("key".to_string(), "placeholder".to_string()),
                ("root-token".to_string(), "placeholder".to_string()),
            ]);
            let _ = k::create_secret("data", "openbao-keys", placeholder).await;
        }

        let _ = secrets::wait_pod_running("data", &ob_pod, 300).await;

        Ok(ExecutionResult::next())
    }
}

// ── InitOrUnsealOpenBao ─────────────────────────────────────────────────────

/// Port-forward to OpenBao, check seal status, init if needed (storing keys
/// in K8s secret), unseal if needed, enable KV engine.
/// Reads `ob_pod`, `skip_seed`. Sets `ob_port`, `root_token`, or `skip_seed`.
#[derive(Default)]
pub struct InitOrUnsealOpenBao;

#[async_trait::async_trait]
impl StepBody for InitOrUnsealOpenBao {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        if ctx.workflow.data.get("skip_seed").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match ctx.workflow.data.get("ob_pod").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(ExecutionResult::next()),
        };

        // Port-forward with retries
        let mut pf = None;
        for attempt in 0..10 {
            match secrets::port_forward("data", &ob_pod, 8200).await {
                Ok(p) => { pf = Some(p); break; }
                Err(e) => {
                    if attempt < 9 {
                        ok(&format!("Waiting for OpenBao to accept connections (attempt {})...", attempt + 1));
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    } else {
                        return Err(step_err(format!(
                            "Port-forward to OpenBao failed after 10 attempts: {e}"
                        )));
                    }
                }
            }
        }
        let pf = pf.unwrap();
        let bao_url = format!("http://127.0.0.1:{}", pf.local_port);
        let bao = BaoClient::new(&bao_url);

        // Wait for API to respond
        let mut status = None;
        for attempt in 0..30 {
            match bao.seal_status().await {
                Ok(s) => { status = Some(s); break; }
                Err(_) if attempt < 29 => {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
                Err(_) => {}
            }
        }

        let mut unseal_key = String::new();
        let mut root_token = String::new();

        let status = status.unwrap_or_else(|| crate::openbao::SealStatusResponse {
            initialized: false, sealed: true, progress: 0, t: 0, n: 0,
        });

        // Check if truly initialized (not just a placeholder secret)
        let mut already_initialized = status.initialized;
        if !already_initialized {
            if let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await {
                if !key.is_empty() && key != "placeholder" {
                    already_initialized = true;
                }
            }
        }

        if !already_initialized {
            ok("Initializing OpenBao...");
            match bao.init(1, 1).await {
                Ok(init) => {
                    unseal_key = init.unseal_keys_b64[0].clone();
                    root_token = init.root_token.clone();
                    let mut secret_data = HashMap::new();
                    secret_data.insert("key".to_string(), unseal_key.clone());
                    secret_data.insert("root-token".to_string(), root_token.clone());
                    k::create_secret("data", "openbao-keys", secret_data).await
                        .map_err(|e| step_err(e.to_string()))?;
                    ok("Initialized -- keys stored in secret/openbao-keys.");
                }
                Err(e) => {
                    warn(&format!("Init failed -- resetting OpenBao storage... ({e})"));
                    let _ = secrets::delete_resource("data", "pvc", "data-openbao-0").await;
                    let _ = secrets::delete_resource("data", "pod", &ob_pod).await;
                    warn("OpenBao storage reset. Run again after the pod restarts.");
                    let mut result = ExecutionResult::next();
                    result.output_data = Some(serde_json::json!({ "skip_seed": true }));
                    return Ok(result);
                }
            }
        } else {
            ok("Already initialized.");
            if let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await {
                if key != "placeholder" { unseal_key = key; }
            }
            if let Ok(token) = k::kube_get_secret_field("data", "openbao-keys", "root-token").await {
                if token != "placeholder" { root_token = token; }
            }
        }

        // Unseal if needed
        let status = bao.seal_status().await.unwrap_or_else(|_| {
            crate::openbao::SealStatusResponse {
                initialized: true, sealed: true, progress: 0, t: 0, n: 0,
            }
        });
        if status.sealed && !unseal_key.is_empty() {
            ok("Unsealing...");
            bao.unseal(&unseal_key).await
                .map_err(|e| step_err(format!("Failed to unseal OpenBao: {e}")))?;
        }

        if root_token.is_empty() {
            warn("No root token available -- skipping vault operations.");
            let mut result = ExecutionResult::next();
            result.output_data = Some(serde_json::json!({ "skip_seed": true }));
            return Ok(result);
        }

        // Enable & tune KV engine
        let bao = BaoClient::with_token(&bao_url, &root_token);
        ok("Enabling KV engine...");
        let _ = bao.enable_secrets_engine("secret", "kv").await;
        let _ = bao
            .write(
                "sys/mounts/secret/tune",
                &serde_json::json!({"options": {"version": "2"}}),
            )
            .await;

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({
            "ob_port": pf.local_port,
            "root_token": root_token,
        }));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::WorkflowStatus;

    async fn run_step<S: StepBody + Default + 'static>(
        data: serde_json::Value,
    ) -> wfe_core::models::WorkflowInstance {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        host.register_step::<S>().await;
        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<S>()
            .name("test-step")
            .end_workflow()
            .build("test-wf", 1);
        host.register_workflow_definition(def).await;
        let instance = run_workflow_sync(&host, "test-wf", 1, data, Duration::from_secs(5))
            .await
            .unwrap();
        host.stop().await;
        instance
    }

    #[tokio::test]
    async fn test_wait_pod_running_skip_seed() {
        let data = serde_json::json!({ "skip_seed": true });
        let instance = run_step::<WaitPodRunning>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_wait_pod_running_no_ob_pod() {
        let data = serde_json::json!({ "skip_seed": false });
        let instance = run_step::<WaitPodRunning>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_wait_pod_running_ob_pod_none_explicit() {
        let data = serde_json::json!({ "skip_seed": false, "ob_pod": null });
        let instance = run_step::<WaitPodRunning>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_init_or_unseal_skip_seed() {
        let data = serde_json::json!({ "skip_seed": true });
        let instance = run_step::<InitOrUnsealOpenBao>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_init_or_unseal_no_ob_pod() {
        let data = serde_json::json!({ "skip_seed": false });
        let instance = run_step::<InitOrUnsealOpenBao>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_init_or_unseal_ob_pod_none() {
        let data = serde_json::json!({ "skip_seed": false, "ob_pod": null });
        let instance = run_step::<InitOrUnsealOpenBao>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }
}
