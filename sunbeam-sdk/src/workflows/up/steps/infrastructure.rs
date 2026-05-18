//! Infrastructure steps: Cilium check, buildkit check.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::{ok, step, warn};
use crate::workflows::data::UpData;

// ── EnsureCilium ────────────────────────────────────────────────────────────

/// Verify Cilium CNI pods are running in kube-system. Warn if missing, don't fail.
#[derive(Default)]
pub struct EnsureCilium;

#[async_trait::async_trait]
impl StepBody for EnsureCilium {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        if data.skip_cilium {
            warn("Skipping Cilium check (--skip-cilium).");
            ok("Cilium check skipped.");
            return Ok(ExecutionResult::next());
        }

        let step_ctx = data
            .ctx
            .as_ref()
            .ok_or_else(|| wfe_core::WfeError::StepExecution("missing __ctx".into()))?;

        // Initialize kube context for the rest of the workflow
        k::set_context(&step_ctx.kube_context);

        step("Cilium CNI...");

        // Cilium may still be installing after Lima VM provisioning; wait up to 5 min.
        // Re-create the client on each attempt so kubeconfig updates are picked up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut found = false;
        loop {
            match k::get_client().await {
                Ok(client) => {
                    found = check_cilium_pods(&client, "kube-system").await
                        || check_cilium_pods(&client, "cilium-system").await;
                    if found {
                        break;
                    }
                }
                Err(e) => {
                    step(&format!("Cilium CNI (waiting for API: {e})..."));
                }
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        if !found {
            return Err(wfe_core::WfeError::StepExecution(
                "Cilium pods not found after 5 min. CNI should be installed at the infrastructure level."
                    .into(),
            ));
        }
        ok("Cilium is healthy.");

        // For local dev, always resolve domain from the live cluster so that
        // VM IP changes (e.g. new Lima instance) are picked up automatically.
        // Production contexts keep their statically configured domain.
        let mut result = ExecutionResult::next();
        if is_local_dev_domain(&data.domain) {
            let live_domain = k::get_domain()
                .await
                .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
            if !live_domain.is_empty() && live_domain != data.domain {
                result.output_data = Some(serde_json::json!({ "domain": live_domain }));
            }
        }

        Ok(result)
    }
}

fn is_local_dev_domain(domain: &str) -> bool {
    domain.is_empty()
        || domain.ends_with("sslip.io")
        || domain.ends_with("nip.io")
        || domain == "localhost"
        || domain.starts_with("192.168.")
        || domain.starts_with("10.")
        || domain.starts_with("172.")
}

async fn check_cilium_pods(client: &kube::Client, ns: &str) -> bool {
    let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
        kube::Api::namespaced(client.clone(), ns);
    let lp = kube::api::ListParams::default().labels("k8s-app=cilium");
    match pods.list(&lp).await {
        Ok(list) => !list.items.is_empty(),
        Err(_) => false,
    }
}

// ── EnsureBuildKit ──────────────────────────────────────────────────────────

/// Check buildkit pods, warn if not present.
#[derive(Default)]
pub struct EnsureBuildKit;

#[async_trait::async_trait]
impl StepBody for EnsureBuildKit {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let _data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        step("BuildKit...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "buildkit");
        let lp = kube::api::ListParams::default();
        match pods.list(&lp).await {
            Ok(list) if !list.items.is_empty() => {
                ok("BuildKit is present.");
                Ok(ExecutionResult::next())
            }
            _ => Err(wfe_core::WfeError::StepExecution(
                "BuildKit pods not found -- image builds may not work.".into(),
            )),
        }
    }
}

// ── EnsureSeaweedFSBuckets ─────────────────────────────────────────────────

/// Create required S3 buckets in SeaweedFS before apps that depend on them start.
#[derive(Default)]
pub struct EnsureSeaweedFSBuckets;

#[async_trait::async_trait]
impl StepBody for EnsureSeaweedFSBuckets {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        step("SeaweedFS buckets...");

        // Wait for the seaweedfs master pod (up to 3 min)
        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "storage");
        let lp = kube::api::ListParams::default()
            .labels("app=seaweedfs-master");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let master_pod = loop {
            let pod_list = pods.list(&lp).await.map_err(|e| {
                wfe_core::WfeError::StepExecution(format!(
                    "Could not list seaweedfs master pods: {e}"
                ))
            })?;
            if let Some(name) = pod_list
                .items
                .first()
                .and_then(|p| p.metadata.name.as_deref())
            {
                break name.to_string();
            }
            if std::time::Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "SeaweedFS master pod not found after 3 min".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        };

        // Create zot bucket via weed shell (piped via kube API exec)
        let bucket_cmd = b"s3.bucket.create -name zot\n";
        let (exit_code, stdout) = k::kube_exec_with_stdin(
            "storage",
            &master_pod,
            &[
                "weed",
                "shell",
                "-master=seaweedfs-master-0.seaweedfs-master:9333",
                "-filer=seaweedfs-filer.storage.svc.cluster.local:8888",
            ],
            None,
            Some(bucket_cmd),
        )
        .await
        .map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to exec weed shell: {e}"))
        })?;

        if exit_code != 0 {
            return Err(wfe_core::WfeError::StepExecution(format!(
                "weed shell exited with code {exit_code}: {stdout}"
            )));
        }
        if stdout.contains("created bucket zot") || stdout.contains("bucket zot already exists") {
            ok("Created zot bucket.");
            Ok(ExecutionResult::next())
        } else {
            Err(wfe_core::WfeError::StepExecution(format!(
                "Unexpected bucket creation output: {stdout}"
            )))
        }
    }
}

// ── WaitForCNPGWebhook ──────────────────────────────────────────────────────

/// Wait for CloudNative PostgreSQL (cnpg) webhook to be ready.
///
/// The `Cluster/postgres` CR requires the cnpg mutating webhook to be available.
/// This polls the cnpg-webhook-service endpoints in the data namespace.
#[derive(Default)]
pub struct WaitForCNPGWebhook;

#[async_trait::async_trait]
impl StepBody for WaitForCNPGWebhook {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        use k8s_openapi::api::core::v1::Endpoints;
        use kube::api::{Api, ListParams};
        use std::time::{Duration, Instant};

        step("Waiting for CNPG webhook...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let eps: Api<Endpoints> = Api::namespaced(client.clone(), "data");
        let deploy_api: Api<k8s_openapi::api::apps::v1::Deployment> =
            Api::namespaced(client.clone(), "data");

        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "Timed out waiting for CNPG webhook".into(),
                ));
            }

            // First check the deployment is ready
            let deploy_ready = match deploy_api.get_opt("cloudnative-pg").await {
                Ok(Some(dep)) => dep
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .map_or(false, |conds| {
                        conds.iter().any(|c| c.type_ == "Available" && c.status == "True")
                    }),
                _ => false,
            };

            if deploy_ready {
                // Then check the webhook service has endpoints
                match eps.get_opt("cnpg-webhook-service").await {
                    Ok(Some(ep)) => {
                        let has_addr = ep
                            .subsets
                            .as_ref()
                            .and_then(|ss| ss.first())
                            .and_then(|s| s.addresses.as_ref())
                            .is_some_and(|a| !a.is_empty());
                        if has_addr {
                            ok("CNPG webhook ready.");
                            return Ok(ExecutionResult::next());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Err(wfe_core::WfeError::StepExecution(format!(
                            "Failed to get CNPG webhook endpoints: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

// ── WaitForLonghornWebhook ──────────────────────────────────────────────────

/// Wait for Longhorn admission webhook to be ready.
///
/// PVC creation requires the Longhorn validating webhook to be available.
/// This polls the longhorn-admission-webhook service endpoints.
#[derive(Default)]
pub struct WaitForLonghornWebhook;

#[async_trait::async_trait]
impl StepBody for WaitForLonghornWebhook {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        use k8s_openapi::api::core::v1::Endpoints;
        use kube::api::{Api, ListParams};
        use std::time::{Duration, Instant};

        step("Waiting for Longhorn webhook...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let eps: Api<Endpoints> = Api::namespaced(client.clone(), "longhorn-system");

        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "Timed out waiting for Longhorn webhook".into(),
                ));
            }

            match eps.get_opt("longhorn-admission-webhook").await {
                Ok(Some(ep)) => {
                    let has_addr = ep
                        .subsets
                        .as_ref()
                        .and_then(|ss| ss.first())
                        .and_then(|s| s.addresses.as_ref())
                        .is_some_and(|a| !a.is_empty());
                    if has_addr {
                        ok("Longhorn webhook ready.");
                        return Ok(ExecutionResult::next());
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(wfe_core::WfeError::StepExecution(format!(
                        "Failed to get Longhorn webhook endpoints: {e}"
                    )));
                }
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_cilium_is_default() {
        let _ = EnsureCilium;
    }

    #[test]
    fn ensure_buildkit_is_default() {
        let _ = EnsureBuildKit;
    }

    #[test]
    fn ensure_seaweedfs_buckets_is_default() {
        let _ = EnsureSeaweedFSBuckets;
    }

    #[test]
    fn wait_for_longhorn_webhook_is_default() {
        let _ = WaitForLonghornWebhook;
    }
}
