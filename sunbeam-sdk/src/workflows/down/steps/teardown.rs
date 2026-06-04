//! Down workflow steps — namespace discovery, deletion, and force-cleanup.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::down::{INFRA_NAMESPACES, APP_NAMESPACES};

use crate::workflows::data::DownData;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

// ── DiscoverNamespaces ──────────────────────────────────────────────────────

/// Discover which Sunbeam-managed namespaces actually exist on the cluster.
///
/// Reads `infra` and `keep_data` from workflow data, filters the static
/// namespace lists, and stores the result in `namespaces_to_delete`.
#[derive(Default)]
pub struct DiscoverNamespaces;

#[async_trait::async_trait]
impl StepBody for DiscoverNamespaces {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: DownData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(format!("DownData parse: {e}")))?;

        let client = crate::kube::get_client()
            .await
            .map_err(|e| step_err(format!("kube client: {e}")))?;

        let ns_api: kube::api::Api<k8s_openapi::api::core::v1::Namespace> =
            kube::api::Api::all(client);
        let existing = ns_api
            .list(&kube::api::ListParams::default())
            .await
            .map_err(|e| step_err(format!("list namespaces: {e}")))?;
        let existing_names: std::collections::HashSet<String> = existing
            .items
            .into_iter()
            .filter_map(|n| n.metadata.name)
            .collect();

        let mut to_delete: Vec<String> = APP_NAMESPACES.iter().map(|s| s.to_string()).collect();

        if data.infra {
            to_delete.extend(INFRA_NAMESPACES.iter().map(|s| s.to_string()));
        }

        if data.keep_data {
            to_delete.retain(|ns| ns != "data");
        }

        to_delete.retain(|ns| existing_names.contains(ns));

        if to_delete.is_empty() {
            tracing::info!("No Sunbeam-managed namespaces found — nothing to delete.");
            return Ok(ExecutionResult::next());
        }

        tracing::info!(
            "Namespaces to delete:\n  {}",
            to_delete.join("\n  ")
        );

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({
            "namespaces_to_delete": to_delete,
        }));
        Ok(result)
    }
}

// ── DeleteNamespaces ────────────────────────────────────────────────────────

/// Delete all namespaces listed in `namespaces_to_delete` workflow data.
#[derive(Default)]
pub struct DeleteNamespaces;

#[async_trait::async_trait]
impl StepBody for DeleteNamespaces {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: DownData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(format!("DownData parse: {e}")))?;

        let to_delete = &data.namespaces_to_delete;
        if to_delete.is_empty() {
            return Ok(ExecutionResult::next());
        }

        let client = crate::kube::get_client()
            .await
            .map_err(|e| step_err(format!("kube client: {e}")))?;
        let ns_api: kube::api::Api<k8s_openapi::api::core::v1::Namespace> =
            kube::api::Api::all(client);
        let dp = kube::api::DeleteParams::background();

        for ns in to_delete {
            tracing::info!("Deleting namespace {ns}...");
            match ns_api.delete(ns, &dp).await {
                Ok(_) => tracing::info!("  {ns} deletion started."),
                Err(kube::Error::Api(ae)) if ae.code == 404 => {
                    tracing::info!("  {ns} already gone.")
                }
                Err(e) => tracing::error!("  Failed to delete {ns}: {e}"),
            }
        }

        Ok(ExecutionResult::next())
    }
}

// ── WaitForTermination ──────────────────────────────────────────────────────

/// Poll until all namespaces in `namespaces_to_delete` are gone.
///
/// Stores any remaining namespaces in `remaining_namespaces` for the
/// force-delete step.
#[derive(Default)]
pub struct WaitForTermination;

#[async_trait::async_trait]
impl StepBody for WaitForTermination {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: DownData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(format!("DownData parse: {e}")))?;

        let to_delete = &data.namespaces_to_delete;
        if to_delete.is_empty() {
            return Ok(ExecutionResult::next());
        }

        let client = crate::kube::get_client()
            .await
            .map_err(|e| step_err(format!("kube client: {e}")))?;
        let ns_api: kube::api::Api<k8s_openapi::api::core::v1::Namespace> =
            kube::api::Api::all(client);

        tracing::info!(msg = "Waiting for namespaces to terminate...", namespaces = ?to_delete);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);

        let mut attempt = 0;
        loop {
            attempt += 1;
            if std::time::Instant::now() > deadline {
                tracing::error!(
                    msg = "Timed out waiting for namespace deletion (5 min). Will try force-delete.",
                    remaining = ?to_delete,
                );
                break;
            }

            let mut remaining = Vec::new();
            for ns in to_delete {
                if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                    remaining.push(ns.clone());
                }
            }

            if remaining.is_empty() {
                tracing::info!(msg = "All namespaces deleted.");
                return Ok(ExecutionResult::next());
            }

            if attempt % 10 == 0 {
                tracing::info!(
                    msg = "Still waiting for namespaces to terminate...",
                    remaining = ?remaining,
                    attempt = attempt,
                    elapsed_secs = attempt * 3,
                );
            }

            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }

        // Store remaining for force-delete step
        let mut remaining = Vec::new();
        for ns in to_delete {
            if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                remaining.push(ns.clone());
            }
        }

        if remaining.is_empty() {
            tracing::info!("All namespaces deleted.");
            return Ok(ExecutionResult::next());
        }

        tracing::error!(
            "Namespaces still terminating: {}",
            remaining.join(", ")
        );

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({
            "remaining_namespaces": remaining,
        }));
        Ok(result)
    }
}

// ── ForceDeleteStuckNamespaces ──────────────────────────────────────────────

/// Remove finalizers from stuck namespaces and patch them to force deletion.
#[derive(Default)]
pub struct ForceDeleteStuckNamespaces;

#[async_trait::async_trait]
impl StepBody for ForceDeleteStuckNamespaces {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: DownData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(format!("DownData parse: {e}")))?;

        let remaining = &data.remaining_namespaces;
        if remaining.is_empty() {
            return Ok(ExecutionResult::next());
        }

        let client = crate::kube::get_client()
            .await
            .map_err(|e| step_err(format!("kube client: {e}")))?;
        let ns_api: kube::api::Api<k8s_openapi::api::core::v1::Namespace> =
            kube::api::Api::all(client.clone());

        for ns in remaining {
            tracing::info!("Force-deleting stuck namespace {ns}...");
            let logger = crate::logger::Logger::new(crate::logger::TracingSink);
            if let Err(e) = crate::down::force_delete_namespace(&logger, client.clone(), ns).await {
                tracing::error!("  Force-delete failed for {ns}: {e}");
            }
        }

        // Brief wait after force-delete
        let mut still_stuck: Vec<String> = remaining.clone();
        for attempt in 0..10 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let mut next_stuck = Vec::new();
            for ns in &still_stuck {
                if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                    next_stuck.push(ns.clone());
                }
            }
            if next_stuck.is_empty() {
                tracing::info!(msg = "All namespaces deleted after force-delete.");
                return Ok(ExecutionResult::next());
            }
            still_stuck = next_stuck;
            if attempt % 3 == 0 && attempt > 0 {
                tracing::info!(
                    msg = "Still waiting for force-deleted namespaces to clear...",
                    remaining = ?still_stuck,
                    attempt = attempt + 1,
                );
            }
        }

        if !still_stuck.is_empty() {
            tracing::error!(
                msg = "Namespaces still stuck after force-delete — manual cleanup may be required.",
                namespaces = %still_stuck.join(", "),
            );
        }

        Ok(ExecutionResult::next())
    }
}

// ── DeleteLimaVm ────────────────────────────────────────────────────────────

/// Delete the Lima VM.
///
/// This is a best-effort step — failure does not block the workflow.
#[derive(Default)]
pub struct DeleteLimaVm;

/// Run `limactl delete <VM_NAME> --force` directly.
#[tracing::instrument]
pub async fn delete_lima_vm() -> Result<(), String> {
    let status = tokio::process::Command::new("limactl")
        .args(["delete", crate::constants::LIMA_VM_NAME, "--force"])
        .status()
        .await
        .map_err(|e| format!("Failed to run limactl delete: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("limactl delete exited with status: {status}"))
    }
}

#[async_trait::async_trait]
impl StepBody for DeleteLimaVm {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let profile = ctx
            .workflow
            .data
            .get("profile")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if profile != "lima" {
            tracing::info!("Profile is not 'lima' — skipping Lima VM management.");
            return Ok(ExecutionResult::next());
        }

        tracing::info!("Deleting Lima VM '{}'...", crate::constants::LIMA_VM_NAME);
        match delete_lima_vm().await {
            Ok(()) => tracing::info!("Lima VM deleted."),
            Err(e) => tracing::error!("{e}"),
        }

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_namespaces_is_default() {
        let _ = DiscoverNamespaces;
    }

    #[test]
    fn delete_namespaces_is_default() {
        let _ = DeleteNamespaces;
    }

    #[test]
    fn wait_for_termination_is_default() {
        let _ = WaitForTermination;
    }

    #[test]
    fn force_delete_stuck_is_default() {
        let _ = ForceDeleteStuckNamespaces;
    }

    #[test]
    fn delete_lima_vm_is_default() {
        let _ = DeleteLimaVm;
    }

    #[test]
    fn delete_lima_vm_fn_exists() {
        // Ensure the standalone delete function is available.
        // Actual limactl invocation is tested in integration tests.
        let _ = delete_lima_vm;
    }
}
