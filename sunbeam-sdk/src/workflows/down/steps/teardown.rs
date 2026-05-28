//! Down workflow steps — namespace discovery, deletion, and force-cleanup.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::down::{INFRA_NAMESPACES, APP_NAMESPACES};
use crate::output::{ok, step, warn};
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
            ok("No Sunbeam-managed namespaces found — nothing to delete.");
            return Ok(ExecutionResult::next());
        }

        step(&format!(
            "Namespaces to delete:\n  {}",
            to_delete.join("\n  ")
        ));

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
            step(&format!("Deleting namespace {ns}..."));
            match ns_api.delete(ns, &dp).await {
                Ok(_) => ok(&format!("  {ns} deletion started.")),
                Err(kube::Error::Api(ae)) if ae.code == 404 => {
                    ok(&format!("  {ns} already gone."))
                }
                Err(e) => warn(&format!("  Failed to delete {ns}: {e}")),
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

        step("Waiting for namespaces to terminate...");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);

        loop {
            if std::time::Instant::now() > deadline {
                warn("Timed out waiting for namespace deletion.");
                break;
            }

            let mut remaining = Vec::new();
            for ns in to_delete {
                if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                    remaining.push(ns.clone());
                }
            }

            if remaining.is_empty() {
                ok("All namespaces deleted.");
                return Ok(ExecutionResult::next());
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
            ok("All namespaces deleted.");
            return Ok(ExecutionResult::next());
        }

        warn(&format!(
            "Namespaces still terminating: {}",
            remaining.join(", ")
        ));

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
            step(&format!("Force-deleting stuck namespace {ns}..."));
            if let Err(e) = crate::down::force_delete_namespace(client.clone(), ns).await {
                warn(&format!("  Force-delete failed for {ns}: {e}"));
            }
        }

        // Brief wait after force-delete
        let mut still_stuck: Vec<String> = remaining.clone();
        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let mut next_stuck = Vec::new();
            for ns in &still_stuck {
                if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                    next_stuck.push(ns.clone());
                }
            }
            if next_stuck.is_empty() {
                ok("All namespaces deleted after force-delete.");
                return Ok(ExecutionResult::next());
            }
            still_stuck = next_stuck;
        }

        if !still_stuck.is_empty() {
            warn(&format!(
                "Namespaces still stuck after force-delete: {}",
                still_stuck.join(", ")
            ));
        }

        Ok(ExecutionResult::next())
    }
}

// ── StopLimaVm ──────────────────────────────────────────────────────────────

/// Stop the Lima sunbeam VM if it exists and is running.
///
/// This is a best-effort step — failure does not block the workflow.
/// Skipped entirely for production domains.
#[derive(Default)]
pub struct StopLimaVm;

#[async_trait::async_trait]
impl StepBody for StopLimaVm {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: DownData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(format!("DownData parse: {e}")))?;

        let domain = data
            .ctx
            .as_ref()
            .map(|c| c.domain.as_str())
            .unwrap_or("");

        let is_local_dev = domain.is_empty()
            || domain.ends_with("sslip.io")
            || domain.ends_with("nip.io")
            || domain == "localhost"
            || domain.starts_with("192.168.")
            || domain.starts_with("10.")
            || domain.starts_with("172.");

        if !is_local_dev {
            ok("Non-local domain — skipping Lima VM management.");
            return Ok(ExecutionResult::next());
        }

        step("Checking Lima VM...");

        let output = tokio::process::Command::new("limactl")
            .args(["list", "sunbeam", "--format", "{{.Status}}"])
            .output()
            .await;

        let status = match output {
            Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
            Err(e) => {
                warn(&format!("Could not query Lima status: {e}"));
                return Ok(ExecutionResult::next());
            }
        };

        if status.is_empty() || status == "None" {
            ok("Lima VM 'sunbeam' does not exist.");
            return Ok(ExecutionResult::next());
        }

        if status == "Running" {
            step("Stopping Lima VM 'sunbeam'...");
            match tokio::process::Command::new("limactl")
                .args(["stop", "sunbeam"])
                .status()
                .await
            {
                Ok(st) if st.success() => ok("Lima VM stopped."),
                Ok(st) => warn(&format!("limactl stop exited with code: {st}")),
                Err(e) => warn(&format!("Failed to stop Lima VM: {e}")),
            }
        } else {
            ok(&format!("Lima VM 'sunbeam' is {status}."));
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
    fn stop_lima_vm_is_default() {
        let _ = StopLimaVm;
    }
}
