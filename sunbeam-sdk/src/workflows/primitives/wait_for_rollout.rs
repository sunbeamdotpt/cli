//! WaitForRollout — atomic step that waits for a single Deployment to be ready.
//!
//! Reads `step_config.namespace`, `step_config.deployment`, and optional `step_config.timeout_secs`.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::{debug, info};

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Wait for a single Deployment rollout to complete.
///
/// **step_config:** `{"namespace": "ory", "deployment": "kratos", "timeout_secs": 120}`
///
/// `timeout_secs` defaults to 120 if omitted.
pub struct WaitForRollout {
    logger: crate::logger::Logger,
}

impl Default for WaitForRollout {
    fn default() -> Self {
        Self {
            logger: crate::logger::Logger::new(crate::logger::TracingSink),
        }
    }
}

#[async_trait::async_trait]
impl StepBody for WaitForRollout {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let logger = &self.logger;
        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("WaitForRollout: missing step_config"))?;
        let namespace = config
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WaitForRollout: missing namespace in step_config"))?;
        let deployment = config
            .get("deployment")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WaitForRollout: missing deployment in step_config"))?;
        debug!(logger, "wait_for_rollout", namespace = namespace, deployment = deployment);
        let timeout_secs = config
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        let serial_mode = ctx
            .workflow
            .data
            .get("serial_mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let timeout_secs = if serial_mode { timeout_secs * 2 } else { timeout_secs };

        let skip_namespaces: Vec<String> = ctx
            .workflow
            .data
            .get("skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if skip_namespaces.contains(&namespace.to_string()) {
            info!(logger, "Skipping wait for rollout (profile skip list)", namespace = namespace, deployment = deployment);
            return Ok(ExecutionResult::next());
        }

        info!(logger, "Waiting for rollout", namespace = namespace, deployment = deployment);

        crate::cluster::wait_rollout(logger, namespace, deployment, timeout_secs)
            .await
            .map_err(|e| step_err(format!("WaitForRollout({namespace}/{deployment}): {e}")))?;
        info!(logger, "Rollout complete.", namespace = namespace, deployment = deployment);

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_for_rollout_is_default() {
        let _ = WaitForRollout::default();
    }

    #[test]
    fn error_includes_context() {
        let err = step_err(format!("WaitForRollout({}/{}): timed out", "ory", "kratos"));
        let msg = err.to_string();
        assert!(
            msg.contains("ory/kratos"),
            "error should include ns/deploy: {msg}"
        );
    }
}
