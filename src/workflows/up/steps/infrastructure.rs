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

        let step_ctx = data
            .ctx
            .as_ref()
            .ok_or_else(|| wfe_core::WfeError::StepExecution("missing __ctx".into()))?;

        // Initialize kube context for the rest of the workflow
        k::set_context(&step_ctx.kube_context, &step_ctx.ssh_host);

        step("Cilium CNI...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        let found = check_cilium_pods(client, "kube-system").await
            || check_cilium_pods(client, "cilium-system").await;

        if found {
            ok("Cilium is healthy.");
        } else {
            warn("Cilium pods not found. CNI should be installed at the infrastructure level.");
            warn("Continuing anyway -- networking may not work correctly.");
        }

        // Resolve domain if empty and store in workflow data
        let mut result = ExecutionResult::next();
        if data.domain.is_empty() {
            let domain = k::get_domain()
                .await
                .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
            result.output_data = Some(serde_json::json!({ "domain": domain }));
        }

        Ok(result)
    }
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
            Ok(list) if !list.items.is_empty() => ok("BuildKit is present."),
            _ => warn("BuildKit pods not found -- image builds may not work."),
        }

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_cilium_is_default() {
        let _ = EnsureCilium::default();
    }

    #[test]
    fn ensure_buildkit_is_default() {
        let _ = EnsureBuildKit::default();
    }
}
