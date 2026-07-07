//! Infrastructure steps: Cilium check, buildkit check.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;

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
            tracing::info!(msg = "Skipping Cilium check (--skip-cilium).");
            tracing::info!(msg = "Cilium check skipped.");
            return Ok(ExecutionResult::next());
        }

        let step_ctx = data
            .ctx
            .as_ref()
            .ok_or_else(|| wfe_core::WfeError::StepExecution("missing __ctx".into()))?;

        // Initialize kube context for the rest of the workflow
        k::set_context(&step_ctx.kube_context);

        tracing::info!(msg = "Checking Cilium CNI pods...");

        // Cilium may still be installing after Lima VM provisioning; wait up to 5 min.
        // Re-create the client on each attempt so kubeconfig updates are picked up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut found = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match k::get_client().await {
                Ok(client) => {
                    let ns_system = check_cilium_pods(&client, "kube-system", attempt).await;
                    let ns_cilium = check_cilium_pods(&client, "cilium-system", attempt).await;
                    found = ns_system || ns_cilium;
                    if found {
                        break;
                    }
                }
                Err(e) => {
                    tracing::info!(
                        msg = "Cilium check waiting for Kubernetes API...",
                        attempt = attempt,
                        err = %e,
                    );
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
        tracing::info!(msg = "Cilium is healthy.");

        // Resolve domain from the live cluster so that VM IP changes
        // (e.g. new Lima instance) are picked up automatically.
        let mut result = ExecutionResult::next();
        let live_domain = k::get_domain()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        if !live_domain.is_empty() && live_domain != data.domain {
            result.output_data = Some(serde_json::json!({ "domain": live_domain }));
        }

        Ok(result)
    }
}

async fn check_cilium_pods(client: &kube::Client, ns: &str, attempt: u32) -> bool {
    let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
        kube::Api::namespaced(client.clone(), ns);
    let lp = kube::api::ListParams::default().labels("k8s-app=cilium");
    match pods.list(&lp).await {
        Ok(list) => {
            let count = list.items.len();
            if !list.items.is_empty() {
                tracing::info!(
                    msg = "Found Cilium pods.",
                    namespace = ns,
                    count = count,
                    attempt = attempt,
                );
            }
            !list.items.is_empty()
        }
        Err(e) => {
            tracing::debug!(
                msg = "Failed to list Cilium pods (transient).",
                namespace = ns,
                attempt = attempt,
                err = %e,
            );
            false
        }
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

        tracing::info!(msg = "Checking BuildKit pods...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "buildkit");
        let lp = kube::api::ListParams::default();
        match pods.list(&lp).await {
            Ok(list) if !list.items.is_empty() => {
                tracing::info!(msg = "BuildKit is present.", count = list.items.len());
                Ok(ExecutionResult::next())
            }
            Ok(list) => Err(wfe_core::WfeError::StepExecution(format!(
                "BuildKit pods not found (count={}) -- image builds may not work.",
                list.items.len()
            ))),
            Err(e) => Err(wfe_core::WfeError::StepExecution(format!(
                "BuildKit pod list failed: {e}"
            ))),
        }
    }
}

// ── EnsureSeaweedFSBuckets ─────────────────────────────────────────────────

/// Create required S3 buckets in SeaweedFS before apps that depend on them start.
#[derive(Default)]
pub struct EnsureSeaweedFSBuckets;

#[async_trait::async_trait]
impl StepBody for EnsureSeaweedFSBuckets {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let skip_namespaces: Vec<String> = ctx
            .workflow
            .data
            .get("skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if skip_namespaces.contains(&"storage".to_string()) {
            tracing::info!("Skipping SeaweedFS bucket setup (profile skip list)");
            return Ok(ExecutionResult::next());
        }

        tracing::info!(msg = "Checking SeaweedFS buckets...");

        // Wait for the seaweedfs master pod (up to 3 min)
        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "storage");
        let lp = kube::api::ListParams::default().labels("app=seaweedfs-master");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut attempt = 0;
        let master_pod = loop {
            attempt += 1;
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
                tracing::info!(
                    msg = "SeaweedFS master pod found.",
                    pod = name,
                    attempt = attempt,
                );
                break name.to_string();
            }
            if std::time::Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "SeaweedFS master pod not found after 3 min".into(),
                ));
            }
            if attempt % 6 == 0 {
                tracing::info!(
                    msg = "Still waiting for SeaweedFS master pod...",
                    attempt = attempt,
                    elapsed_secs = attempt * 5,
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        };

        // Create zot bucket via weed shell (piped via kube API exec)
        let bucket_cmd = b"s3.bucket.create -name zot\n";
        tracing::info!(
            msg = "Creating zot S3 bucket via weed shell...",
            pod = %master_pod,
        );
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
            wfe_core::WfeError::StepExecution(format!(
                "Failed to exec weed shell in pod {master_pod}: {e}"
            ))
        })?;

        if exit_code != 0 {
            return Err(wfe_core::WfeError::StepExecution(format!(
                "weed shell exited with code {exit_code}: {stdout}"
            )));
        }
        if stdout.contains("created bucket zot") || stdout.contains("bucket zot already exists") {
            tracing::info!(msg = "zot bucket ready.");
            Ok(ExecutionResult::next())
        } else {
            Err(wfe_core::WfeError::StepExecution(format!(
                "Unexpected bucket creation output from weed shell: {stdout}"
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
        use kube::api::Api;
        use std::time::{Duration, Instant};

        tracing::info!(msg = "Waiting for CNPG webhook...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let eps: Api<Endpoints> = Api::namespaced(client.clone(), "data");
        let deploy_api: Api<k8s_openapi::api::apps::v1::Deployment> =
            Api::namespaced(client.clone(), "data");

        let deadline = Instant::now() + Duration::from_secs(180);
        let mut attempt = 0;
        loop {
            attempt += 1;
            if Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "Timed out waiting for CNPG webhook (3 min). Check: kubectl get pods -n data"
                        .into(),
                ));
            }

            // First check the deployment is ready
            let deploy_ready = match deploy_api.get_opt("cloudnative-pg").await {
                Ok(Some(dep)) => dep
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|conds| {
                        conds
                            .iter()
                            .any(|c| c.type_ == "Available" && c.status == "True")
                    }),
                Ok(None) => false,
                Err(e) => {
                    return Err(wfe_core::WfeError::StepExecution(format!(
                        "Failed to get CNPG deployment in namespace data: {e}"
                    )));
                }
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
                            tracing::info!(msg = "CNPG webhook ready.");
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

            if attempt % 10 == 0 {
                tracing::info!(
                    msg = "Still waiting for CNPG webhook...",
                    attempt = attempt,
                    deployment_ready = deploy_ready,
                );
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

// ── WaitForLonghornWebhook ──────────────────────────────────────────────────

/// Check whether a DaemonSet status indicates readiness.
fn is_daemonset_ready(status: &k8s_openapi::api::apps::v1::DaemonSetStatus) -> bool {
    let desired = status.desired_number_scheduled;
    let ready = status.number_ready;
    let available = status.number_available.unwrap_or(0);
    desired > 0 && ready >= desired && available >= desired
}

/// Wait for Longhorn admission webhook to be ready.
///
/// In Longhorn v1.11.1 the webhook was folded into the manager DaemonSet, but
/// the `longhorn-admission-webhook` Service selector was not updated, so that
/// Service will never have endpoints. We therefore wait for the
/// `longhorn-manager` DaemonSet to be ready instead.
#[derive(Default)]
pub struct WaitForLonghornWebhook;

#[async_trait::async_trait]
impl StepBody for WaitForLonghornWebhook {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        use k8s_openapi::api::apps::v1::DaemonSet;
        use kube::api::Api;
        use std::time::{Duration, Instant};

        tracing::info!(msg = "Waiting for Longhorn manager (webhook) ...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let ds_api: Api<DaemonSet> = Api::namespaced(client.clone(), "longhorn-system");

        let deadline = Instant::now() + Duration::from_secs(180);
        let mut attempt = 0;
        loop {
            attempt += 1;
            if Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "Timed out waiting for Longhorn manager DaemonSet (3 min). Check: kubectl get ds -n longhorn-system".into(),
                ));
            }

            match ds_api.get_opt("longhorn-manager").await {
                Ok(Some(ds)) => {
                    if let Some(status) = &ds.status
                        && is_daemonset_ready(status)
                    {
                        tracing::info!(msg = "Longhorn manager DaemonSet ready.");
                        return Ok(ExecutionResult::next());
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(wfe_core::WfeError::StepExecution(format!(
                        "Failed to get Longhorn manager DaemonSet: {e}"
                    )));
                }
            }

            if attempt % 10 == 0 {
                tracing::info!(
                    msg = "Still waiting for Longhorn manager DaemonSet...",
                    attempt = attempt,
                );
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

    #[test]
    fn daemonset_ready_when_desired_met() {
        use k8s_openapi::api::apps::v1::DaemonSetStatus;
        let status = DaemonSetStatus {
            desired_number_scheduled: 3,
            number_ready: 3,
            number_available: Some(3),
            ..Default::default()
        };
        assert!(is_daemonset_ready(&status));
    }

    #[test]
    fn daemonset_not_ready_when_insufficient_ready() {
        use k8s_openapi::api::apps::v1::DaemonSetStatus;
        let status = DaemonSetStatus {
            desired_number_scheduled: 3,
            number_ready: 2,
            number_available: Some(3),
            ..Default::default()
        };
        assert!(!is_daemonset_ready(&status));
    }

    #[test]
    fn daemonset_not_ready_when_insufficient_available() {
        use k8s_openapi::api::apps::v1::DaemonSetStatus;
        let status = DaemonSetStatus {
            desired_number_scheduled: 3,
            number_ready: 3,
            number_available: Some(2),
            ..Default::default()
        };
        assert!(!is_daemonset_ready(&status));
    }

    #[test]
    fn daemonset_not_ready_when_zero_desired() {
        use k8s_openapi::api::apps::v1::DaemonSetStatus;
        let status = DaemonSetStatus {
            desired_number_scheduled: 0,
            number_ready: 0,
            number_available: Some(0),
            ..Default::default()
        };
        assert!(!is_daemonset_ready(&status));
    }

    #[test]
    fn daemonset_ready_fallbacks_to_zero_when_available_unset() {
        use k8s_openapi::api::apps::v1::DaemonSetStatus;
        let status = DaemonSetStatus {
            desired_number_scheduled: 1,
            number_ready: 1,
            number_available: None,
            ..Default::default()
        };
        // number_available defaults to 0, so 0 >= 1 is false
        assert!(!is_daemonset_ready(&status));
    }
}
