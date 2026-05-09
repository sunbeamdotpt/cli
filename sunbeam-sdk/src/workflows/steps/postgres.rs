//! PostgreSQL setup steps: wait for CNPG cluster, create roles/databases,
//! configure OpenBao database secrets engine.
//!
//! Data-struct-agnostic — reads JSON fields directly for cross-workflow reuse.

use std::collections::HashMap;

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::{ok, warn};
use crate::secrets;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

fn json_bool(data: &serde_json::Value, key: &str) -> bool {
    data.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn json_str(data: &serde_json::Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ── Pure helpers (testable without K8s) ─────────────────────────────────────

/// Build the user to database mapping used by EnsurePGRolesAndDatabases.
pub(crate) fn pg_db_map() -> HashMap<&'static str, &'static str> {
    [
        ("kratos", "kratos_db"),
        ("hydra", "hydra_db"),
        ("penpot", "penpot_db"),
        ("stalwart", "stalwart_db"),
        ("headscale", "headscale_db"),
        ("wfe", "wfe_db"),
        ("typst_editor", "typst_editor_db"),
    ]
    .into_iter()
    .collect()
}

/// SQL to idempotently create a postgres user if it does not exist.
pub(crate) fn ensure_user_sql(user: &str) -> String {
    format!(
        "DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='{user}') \
         THEN CREATE USER {user}; END IF; END $$;"
    )
}

/// SQL to create a database owned by the given user.
pub(crate) fn create_db_sql(db: &str, user: &str) -> String {
    format!("CREATE DATABASE {db} OWNER {user};")
}

// ── WaitForPostgres ─────────────────────────────────────────────────────────

/// Wait for CNPG cluster healthy state, set `pg_pod`.
///
/// Reads: `skip_seed`
/// Writes: `pg_pod`
#[derive(Default)]
pub struct WaitForPostgres;

#[async_trait::async_trait]
impl StepBody for WaitForPostgres {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        if json_bool(&ctx.workflow.data, "skip_seed") {
            return Ok(ExecutionResult::next());
        }

        ok("Waiting for postgres cluster...");
        let mut pg_pod = String::new();

        let client = k::get_client().await.map_err(|e| step_err(e.to_string()))?;
        let ar = ApiResource {
            group: "postgresql.cnpg.io".into(),
            version: "v1".into(),
            api_version: "postgresql.cnpg.io/v1".into(),
            kind: "Cluster".into(),
            plural: "clusters".into(),
        };
        let cnpg_api: Api<DynamicObject> = Api::namespaced_with(client.clone(), "data", &ar);

        for _ in 0..60 {
            if let Ok(cluster) = cnpg_api.get("postgres").await {
                let phase = cluster
                    .data
                    .get("status")
                    .and_then(|s| s.get("phase"))
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                if phase == "Cluster in healthy state" {
                    let pods: Api<Pod> = Api::namespaced(client.clone(), "data");
                    let lp = ListParams::default().labels("cnpg.io/cluster=postgres,role=primary");
                    if let Ok(pod_list) = pods.list(&lp).await
                        && let Some(name) = pod_list
                            .items
                            .first()
                            .and_then(|p| p.metadata.name.as_deref())
                    {
                        pg_pod = name.to_string();
                        ok(&format!("Postgres ready ({pg_pod})."));
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        if pg_pod.is_empty() {
            warn("Postgres not ready after 5 min -- continuing anyway.");
        }

        let mut result = ExecutionResult::next();
        if !pg_pod.is_empty() {
            result.output_data = Some(serde_json::json!({ "pg_pod": pg_pod }));
        }
        Ok(result)
    }
}

// ── ConfigureDatabaseEngine ─────────────────────────────────────────────────

/// Configure OpenBao database secrets engine.
///
/// Reads: `skip_seed`, `pg_pod`, `ob_pod`, `root_token`
#[derive(Default)]
pub struct ConfigureDatabaseEngine;

#[async_trait::async_trait]
impl StepBody for ConfigureDatabaseEngine {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if json_bool(data, "skip_seed") {
            return Ok(ExecutionResult::next());
        }

        let _pg_pod = match json_str(data, "pg_pod") {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(ExecutionResult::next()),
        };
        let ob_pod = match json_str(data, "ob_pod") {
            Some(p) => p,
            None => {
                warn("Skipping DB engine config -- missing ob_pod.");
                return Ok(ExecutionResult::next());
            }
        };
        let root_token = match json_str(data, "root_token") {
            Some(t) if !t.is_empty() => t,
            _ => {
                warn("Skipping DB engine config -- missing root_token.");
                return Ok(ExecutionResult::next());
            }
        };

        match secrets::port_forward("data", &ob_pod, 8200).await {
            Ok(pf) => {
                let bao = BaoClient::with_token(
                    &format!("http://127.0.0.1:{}", pf.local_port),
                    &root_token,
                );
                if let Err(e) = secrets::configure_db_engine(&bao).await {
                    warn(&format!("DB engine config failed: {e}"));
                }
            }
            Err(e) => warn(&format!("Port-forward to OpenBao failed: {e}")),
        }

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::WorkflowStatus;

    async fn run_step<S: StepBody + Default + 'static>(
        data: serde_json::Value,
    ) -> wfe_core::models::WorkflowInstance {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        host.register_step::<S>().await;
        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<S>()
            .name("test-step")
            .end_workflow()
            .build("test-wf", 1);
        host.register_workflow_definition(def).await;
        let instance = run_workflow_sync(&host, "test-wf", 1, data, Duration::from_secs(5))
            .await
            .unwrap();
        host.stop().await;
        instance
    }

    #[tokio::test]
    async fn test_wait_for_postgres_skip_seed() {
        let instance = run_step::<WaitForPostgres>(serde_json::json!({ "skip_seed": true })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_configure_db_engine_skip_seed() {
        let instance =
            run_step::<ConfigureDatabaseEngine>(serde_json::json!({ "skip_seed": true })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_configure_db_engine_no_pg_pod() {
        let instance =
            run_step::<ConfigureDatabaseEngine>(serde_json::json!({ "skip_seed": false })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[test]
    fn test_pg_db_map_contains_all_users() {
        let map = pg_db_map();
        assert_eq!(map.len(), 7);
        for user in crate::secrets::PG_USERS {
            assert!(map.contains_key(user), "pg_db_map missing key for: {user}");
        }
    }

    #[test]
    fn test_ensure_user_sql_format() {
        let sql = ensure_user_sql("kratos");
        assert!(sql.contains("rolname='kratos'"));
        assert!(sql.contains("CREATE USER kratos"));
    }

    #[test]
    fn test_create_db_sql_format() {
        assert_eq!(
            create_db_sql("kratos_db", "kratos"),
            "CREATE DATABASE kratos_db OWNER kratos;"
        );
    }
}
