//! EnsureNamespace — atomic step that ensures a Kubernetes namespace exists.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Ensure a single Kubernetes namespace exists (idempotent).
///
/// **step_config:** `{"namespace": "ory"}`
#[derive(Default)]
pub struct EnsureNamespace;

#[async_trait::async_trait]
impl StepBody for EnsureNamespace {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("EnsureNamespace: missing step_config"))?;
        let namespace = config
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("EnsureNamespace: missing namespace in step_config"))?;

        k::ensure_ns(namespace)
            .await
            .map_err(|e| step_err(format!("EnsureNamespace({namespace}): {e}")))?;

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_namespace_is_default() {
        let _ = EnsureNamespace::default();
    }
}
