//! CreatePGRole — atomic step that creates a single PostgreSQL role.
//!
//! Reads `step_config.username` and `pg_pod` from workflow data.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;

use crate::workflows::steps::postgres::ensure_user_sql;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Create a single PostgreSQL role (idempotent).
///
/// **step_config:** `{"username": "kratos"}`
///
/// Reads `pg_pod` and `skip_seed` from workflow data.
#[derive(Default)]
pub struct CreatePGRole;

#[async_trait::async_trait]
impl StepBody for CreatePGRole {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(ExecutionResult::next());
        }

        let pg_pod = match data.get("pg_pod").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(ExecutionResult::next()),
        };

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("CreatePGRole: missing step_config"))?;
        let username = config
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("CreatePGRole: missing username in step_config"))?;

        let sql = ensure_user_sql(username);
        let _ = k::kube_exec(
            "data",
            pg_pod,
            &["psql", "-U", "postgres", "-c", &sql],
            Some("postgres"),
        )
        .await;

        tracing::info!("PG role: {username}");
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_pg_role_is_default() {
        let _ = CreatePGRole;
    }
}
