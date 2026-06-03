//! OpenSearch ML primitives — register/deploy ML model and inject model ID.
//!
//! These run in a parallel branch alongside the main pipeline so the
//! 10+ minute ML model download doesn't block everything else.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};



/// Register and deploy the OpenSearch ML model (all-mpnet-base-v2).
///
/// No step_config needed — reads nothing from workflow data.
/// Can take 10+ minutes on first run (model download).
#[derive(Default)]
pub struct EnsureOpenSearchML;

#[async_trait::async_trait]
impl StepBody for EnsureOpenSearchML {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::info!(msg = "Ensuring OpenSearch ML model...");
        crate::manifests::ensure_opensearch_ml().await;
        tracing::info!(msg = "OpenSearch ML model ready.");
        Ok(ExecutionResult::next())
    }
}

/// Inject the OpenSearch model_id into the matrix/opensearch-ml-config ConfigMap.
///
/// Should run after both the ML model is deployed AND matrix manifests are applied.
#[derive(Default)]
pub struct InjectOpenSearchModelId;

#[async_trait::async_trait]
impl StepBody for InjectOpenSearchModelId {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::info!(msg = "Injecting OpenSearch model ID...");
        crate::manifests::inject_opensearch_model_id().await;
        tracing::info!(msg = "OpenSearch model ID injected.");
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_opensearch_ml_is_default() {
        let _ = EnsureOpenSearchML;
    }

    #[test]
    fn inject_opensearch_model_id_is_default() {
        let _ = InjectOpenSearchModelId;
    }
}
