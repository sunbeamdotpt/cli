//! Platform steps: Gitea bootstrap.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::output::step;

/// Run Gitea bootstrap (repos, webhooks, etc.).
#[derive(Default)]
pub struct BootstrapGitea;

#[async_trait::async_trait]
impl StepBody for BootstrapGitea {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        step("Gitea bootstrap...");
        crate::gitea::cmd_bootstrap()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_gitea_is_default() {
        let _ = BootstrapGitea::default();
    }
}
