//! CreatePGDatabase — atomic step that creates a single PostgreSQL database.
//!
//! Reads `step_config.dbname`, `step_config.owner`, and `pg_pod` from workflow data.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::ok;
use crate::workflows::seed::steps::postgres::create_db_sql;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Create a single PostgreSQL database (idempotent — CREATE DATABASE errors are ignored).
///
/// **step_config:** `{"dbname": "kratos_db", "owner": "kratos"}`
///
/// Reads `pg_pod` and `skip_seed` from workflow data.
#[derive(Default)]
pub struct CreatePGDatabase;

#[async_trait::async_trait]
impl StepBody for CreatePGDatabase {
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
            .ok_or_else(|| step_err("CreatePGDatabase: missing step_config"))?;
        let dbname = config
            .get("dbname")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("CreatePGDatabase: missing dbname in step_config"))?;
        let owner = config
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("CreatePGDatabase: missing owner in step_config"))?;

        let sql = create_db_sql(dbname, owner);
        // kube_exec runs the command inside a Kubernetes pod, not a local shell
        let _ = k::kube_exec(
            "data",
            pg_pod,
            &["psql", "-U", "postgres", "-c", &sql],
            Some("postgres"),
        )
        .await;

        ok(&format!("PG database: {dbname} (owner: {owner})"));
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_pg_database_is_default() {
        let _ = CreatePGDatabase::default();
    }
}
