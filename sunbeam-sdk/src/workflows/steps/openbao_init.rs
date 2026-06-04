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

use crate::secrets;
use crate::vault_keystore::{keystore_exists, load_keystore, save_keystore, VaultKeystore};
use crate::workflows::StepContext;
use chrono::Utc;

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
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let step_ctx: StepContext =
            serde_json::from_value(ctx.workflow.data.get("__ctx").cloned().unwrap_or_default())
                .map_err(|e| step_err(e.to_string()))?;

        k::set_context(&step_ctx.kube_context);

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
                tracing::info!("OpenBao ({name})...");
                result.output_data = Some(serde_json::json!({ "ob_pod": name }));
            }
            None => {
                tracing::info!("OpenBao pod not found -- skipping.");
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
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        if ctx
            .workflow
            .data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            tracing::info!(msg = "Skipping OpenBao pod wait (skip_seed).");
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match ctx.workflow.data.get("ob_pod").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                tracing::info!(msg = "Skipping OpenBao pod wait (no ob_pod).");
                return Ok(ExecutionResult::next());
            }
        };
        tracing::info!(msg = "Waiting for OpenBao pod...", pod = %ob_pod);

        // Ensure openbao-keys secret exists (even as placeholder) so the pod
        // can mount it. InitOrUnsealOpenBao will overwrite with real values.
        if k::kube_get_secret_field("data", "openbao-keys", "key")
            .await
            .is_err()
        {
            let placeholder = std::collections::HashMap::from([
                ("key".to_string(), "placeholder".to_string()),
                ("root-token".to_string(), "placeholder".to_string()),
            ]);
            let _ = k::create_secret("data", "openbao-keys", placeholder).await;
        }

        let _ = secrets::wait_pod_running("data", &ob_pod, 300).await;
        tracing::info!(msg = "OpenBao pod is running.", pod = %ob_pod);

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
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        if ctx
            .workflow
            .data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match ctx.workflow.data.get("ob_pod").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(ExecutionResult::next()),
        };

        let step_ctx: StepContext =
            serde_json::from_value(ctx.workflow.data.get("__ctx").cloned().unwrap_or_default())
                .map_err(|e| step_err(e.to_string()))?;
        let domain = step_ctx.domain;

        // Port-forward with retries
        let mut pf = None;
        for attempt in 0..10 {
            match secrets::port_forward("data", &ob_pod, 8200).await {
                Ok(p) => {
                    pf = Some(p);
                    break;
                }
                Err(e) => {
                    if attempt < 9 {
                        tracing::info!(
                            "Waiting for OpenBao to accept connections (attempt {})...",
                            attempt + 1
                        );
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
        let mut bao_url = format!("http://127.0.0.1:{}", pf.local_port);
        let mut bao = BaoClient::new(&bao_url);

        // Wait for API to respond
        let mut status = None;
        for attempt in 0..30 {
            match bao.seal_status().await {
                Ok(s) => {
                    status = Some(s);
                    break;
                }
                Err(e) if attempt < 29 => {
                    if attempt % 5 == 0 {
                        tracing::info!(
                            msg = "Waiting for OpenBao API to respond...",
                            attempt = attempt + 1,
                            err = %e,
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
                Err(e) => {
                    tracing::error!(
                        msg = "OpenBao API did not respond after 30 attempts.",
                        err = %e,
                    );
                }
            }
        }

        let mut unseal_key = String::new();
        let mut root_token = String::new();

        let status = status.unwrap_or(crate::openbao::SealStatusResponse {
            initialized: false,
            sealed: true,
        });

        // Check if truly initialized (not just a placeholder secret)
        let mut already_initialized = status.initialized;
        if !already_initialized
            && let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await
            && !key.is_empty()
            && key != "placeholder"
        {
            already_initialized = true;
        }

        // Load local keystore as fallback
        let local_keystore = if !domain.is_empty() {
            load_keystore(&domain).ok()
        } else {
            None
        };

        if already_initialized {
            tracing::info!("Already initialized.");
            if let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await
                && key != "placeholder"
            {
                unseal_key = key;
            }
            if let Ok(token) = k::kube_get_secret_field("data", "openbao-keys", "root-token").await
                && token != "placeholder"
            {
                root_token = token;
            }

            // If cluster secret is missing keys but local keystore has them, restore
            if (root_token.is_empty() || unseal_key.is_empty()) && local_keystore.is_some() {
                let ks = local_keystore.as_ref().unwrap();
                if !ks.root_token.is_empty() && !ks.unseal_keys_b64.is_empty() {
                    tracing::error!("Cluster secret missing keys — restoring from local keystore...");
                    let mut secret_data = HashMap::new();
                    secret_data.insert("key".to_string(), ks.unseal_keys_b64[0].clone());
                    secret_data.insert("root-token".to_string(), ks.root_token.clone());
                    k::create_secret("data", "openbao-keys", secret_data)
                        .await
                        .map_err(|e| step_err(e.to_string()))?;
                    unseal_key = ks.unseal_keys_b64[0].clone();
                    root_token = ks.root_token.clone();
                    tracing::info!("Cluster secret restored from local keystore.");
                }
            }

            // If vault is initialized but we lost the root token, reset storage
            // and wait for the pod to restart so we can re-initialize inline.
            if root_token.is_empty() {
                tracing::error!("Vault is initialized but root token is missing -- resetting storage...");
                let _ = secrets::delete_resource("data", "pvc", "data-openbao-0").await;
                let _ = secrets::delete_resource("data", "pod", &ob_pod).await;
                tracing::info!("Waiting for OpenBao pod to restart...");

                // Poll for a new openbao pod to reach Running (up to 5 min).
                let client = k::get_client().await.map_err(|e| step_err(e.to_string()))?;
                let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
                    kube::Api::namespaced(client.clone(), "data");
                let lp = kube::api::ListParams::default()
                    .labels("app.kubernetes.io/name=openbao,component=server");
                let mut new_pod = String::new();
                for attempt in 0..60 {
                    if let Ok(pod_list) = pods.list(&lp).await {
                        if let Some(pod) = pod_list.items.first() {
                            if let Some(name) = pod.metadata.name.as_deref() {
                                if let Some(phase) =
                                    pod.status.as_ref().and_then(|s| s.phase.as_deref())
                                {
                                    if phase == "Running" {
                                        new_pod = name.to_string();
                                        tracing::info!(
                                            msg = "OpenBao restarted.",
                                            pod = %new_pod,
                                            attempt = attempt + 1,
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if attempt % 6 == 0 && attempt > 0 {
                        tracing::info!(
                            msg = "Still waiting for OpenBao pod to restart...",
                            attempt = attempt + 1,
                            elapsed_secs = (attempt + 1) * 5,
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                if new_pod.is_empty() {
                    return Err(step_err("OpenBao pod did not restart after storage reset"));
                }

                // Re-establish port-forward to the new pod.
                let mut pf2 = None;
                for attempt in 0..10 {
                    match secrets::port_forward("data", &new_pod, 8200).await {
                        Ok(p) => {
                            pf2 = Some(p);
                            break;
                        }
                        Err(_) if attempt < 9 => {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                        Err(e) => {
                            return Err(step_err(format!(
                                "Port-forward to restarted OpenBao failed: {e}"
                            )));
                        }
                    }
                }
                let pf2 = pf2.unwrap();
                bao_url = format!("http://127.0.0.1:{}", pf2.local_port);
                bao = BaoClient::new(&bao_url);

                // Wait for API and confirm uninitialized.
                for attempt in 0..30 {
                    if let Ok(s) = bao.seal_status().await {
                        if !s.initialized {
                            tracing::info!("OpenBao is fresh (uninitialized).");
                            break;
                        }
                    }
                    if attempt < 29 {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                }

                // Now fall through to the initialization block below.
                // We set already_initialized = false so the next block runs.
                already_initialized = false;
            }
        }

        if !already_initialized {
            tracing::info!("Initializing OpenBao...");
            let mut init_err = None;
            let mut init_result = None;
            for attempt in 0..5 {
                match bao.init(1, 1).await {
                    Ok(init) => {
                        init_result = Some(init);
                        break;
                    }
                    Err(e) => {
                        init_err = Some(e);
                        if attempt < 4 {
                            tracing::error!(
                                "OpenBao init attempt {} failed, retrying in {}s...",
                                attempt + 1,
                                3 + attempt * 2
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(
                                3 + attempt as u64 * 2,
                            ))
                            .await;
                        }
                    }
                }
            }
            match init_result {
                Some(init) => {
                    unseal_key = init.keys_base64[0].clone();
                    root_token = init.root_token.clone();
                    let mut secret_data = HashMap::new();
                    secret_data.insert("key".to_string(), unseal_key.clone());
                    secret_data.insert("root-token".to_string(), root_token.clone());
                    k::create_secret("data", "openbao-keys", secret_data)
                        .await
                        .map_err(|e| step_err(e.to_string()))?;
                    tracing::info!("Initialized -- keys stored in secret/openbao-keys.");

                    // Save to local keystore
                    if !domain.is_empty() {
                        let ks = VaultKeystore {
                            version: 1,
                            domain: domain.clone(),
                            created_at: Utc::now(),
                            updated_at: Utc::now(),
                            root_token: root_token.clone(),
                            unseal_keys_b64: vec![unseal_key.clone()],
                            key_shares: 1,
                            key_threshold: 1,
                        };
                        if let Err(e) = save_keystore(&ks) {
                            tracing::error!("Failed to save vault keystore: {e}");
                        } else {
                            tracing::info!("Keys saved to local keystore.");
                        }
                    }
                }
                None => {
                    return Err(step_err(format!(
                        "OpenBao init failed after 5 attempts: {}. Manual fix required: check pod logs and network connectivity.",
                        init_err.unwrap()
                    )));
                }
            }
        }

        // Unseal if needed
        let status = bao
            .seal_status()
            .await
            .unwrap_or(crate::openbao::SealStatusResponse {
                initialized: true,
                sealed: true,
            });
        if status.sealed && !unseal_key.is_empty() {
            tracing::info!("Unsealing...");
            bao.unseal(&unseal_key)
                .await
                .map_err(|e| step_err(format!("Failed to unseal OpenBao: {e}")))?;
        }

        // If we read keys from cluster but local keystore is missing, backfill
        if already_initialized && !root_token.is_empty() && !domain.is_empty() && !keystore_exists(&domain) {
            let ks = VaultKeystore {
                version: 1,
                domain: domain.clone(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                root_token: root_token.clone(),
                unseal_keys_b64: vec![unseal_key.clone()],
                key_shares: 1,
                key_threshold: 1,
            };
            if let Err(e) = save_keystore(&ks) {
                tracing::error!("Failed to backfill vault keystore: {e}");
            } else {
                tracing::info!("Local keystore backfilled from cluster secret.");
            }
        }

        // Enable & tune KV engine
        let bao = BaoClient::with_token(&bao_url, &root_token);
        tracing::info!("Enabling KV engine...");
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
