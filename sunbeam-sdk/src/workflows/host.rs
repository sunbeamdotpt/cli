//! Workflow host construction and lifecycle.

use std::path::PathBuf;
use std::sync::Arc;

use wfe::WorkflowHostBuilder;
use wfe_core::test_support::{InMemoryLockProvider, InMemoryQueueProvider};
use wfe_sqlite::SqlitePersistenceProvider;

use crate::error::{Result, SunbeamError};
use crate::info;

/// Build and start a WorkflowHost with a SQLite database at the given path.
///
/// Lock and queue providers are in-memory (single-process, non-distributed).
pub async fn create_host_at(db_path: &std::path::Path) -> Result<wfe::WorkflowHost> {
    let logger = crate::logger::Logger::new(crate::logger::TracingSink);
    info!(
        logger,
        "Opening workflow database...",
        path = db_path.display()
    );
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SunbeamError::Io {
            context: format!("create workflow db dir: {}", parent.display()),
            source: e,
        })?;
    }

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let persistence = SqlitePersistenceProvider::new(&db_url)
        .await
        .map_err(|e| SunbeamError::Other(format!("workflow db init: {e}")))?;

    let host = WorkflowHostBuilder::new()
        .use_persistence(Arc::new(persistence))
        .use_lock_provider(Arc::new(InMemoryLockProvider::new()))
        .use_queue_provider(Arc::new(InMemoryQueueProvider::new()))
        .build()
        .map_err(|e| SunbeamError::Other(format!("workflow host build: {e}")))?;

    host.start()
        .await
        .map_err(|e| SunbeamError::Other(format!("workflow host start: {e}")))?;

    Ok(host)
}

/// Build and start a WorkflowHost configured for the given context.
///
/// The host uses a per-context SQLite database at `~/.sunbeam/{context}/workflows.db`.
pub async fn create_host(context_name: &str) -> Result<wfe::WorkflowHost> {
    let logger = crate::logger::Logger::new(crate::logger::TracingSink);
    info!(logger, "Creating workflow host...", context = context_name);
    let db_path = workflow_db_path(context_name);
    create_host_at(&db_path).await
}

/// Gracefully shut down the host.
pub async fn shutdown_host(host: wfe::WorkflowHost) {
    let logger = crate::logger::Logger::new(crate::logger::TracingSink);
    info!(logger, "Shutting down workflow host...");
    host.stop().await;
}

/// Create a host backed by an in-memory SQLite database (for tests).
#[tracing::instrument]
pub async fn create_test_host() -> Result<wfe::WorkflowHost> {
    let persistence = SqlitePersistenceProvider::new("sqlite::memory:")
        .await
        .map_err(|e| SunbeamError::Other(format!("in-memory db init: {e}")))?;

    let host = WorkflowHostBuilder::new()
        .use_persistence(Arc::new(persistence))
        .use_lock_provider(Arc::new(InMemoryLockProvider::new()))
        .use_queue_provider(Arc::new(InMemoryQueueProvider::new()))
        .build()
        .map_err(|e| SunbeamError::Other(format!("test host build: {e}")))?;

    host.start()
        .await
        .map_err(|e| SunbeamError::Other(format!("test host start: {e}")))?;

    Ok(host)
}

/// Resolve the SQLite database path for a context.
pub fn workflow_db_path(context_name: &str) -> PathBuf {
    crate::config::context_dir(context_name).join("workflows.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::{ExecutionResult, WorkflowStatus};
    use wfe_core::traits::{StepBody, StepExecutionContext};

    #[derive(Default)]
    struct NoOp;
    #[async_trait::async_trait]
    impl StepBody for NoOp {
        async fn run(
            &mut self,
            _ctx: &StepExecutionContext<'_>,
        ) -> wfe_core::Result<ExecutionResult> {
            Ok(ExecutionResult::next())
        }
    }

    #[test]
    fn test_workflow_db_path_default() {
        let path = workflow_db_path("");
        assert!(path.ends_with(".sunbeam/default/workflows.db"));
    }

    #[test]
    fn test_workflow_db_path_named() {
        let path = workflow_db_path("production");
        assert!(path.ends_with(".sunbeam/production/workflows.db"));
    }

    #[test]
    fn test_workflow_db_path_custom() {
        let path = workflow_db_path("staging");
        assert!(path.ends_with(".sunbeam/staging/workflows.db"));
        assert!(!path.to_string_lossy().contains("default"));
    }

    #[tokio::test]
    async fn test_create_test_host() {
        let host = create_test_host().await.unwrap();
        let now = chrono::Utc::now();
        let ids = host
            .persistence()
            .get_runnable_instances(now)
            .await
            .unwrap();
        assert!(ids.is_empty());
        host.stop().await;
    }

    #[tokio::test]
    async fn test_create_host_at_with_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("ctx").join("workflows.db");
        let host = create_host_at(&db_path).await.unwrap();

        // DB file should be created
        assert!(db_path.exists());

        // Should be queryable
        let now = chrono::Utc::now();
        let ids = host
            .persistence()
            .get_runnable_instances(now)
            .await
            .unwrap();
        assert!(ids.is_empty());

        shutdown_host(host).await;
    }

    #[tokio::test]
    async fn test_create_host_at_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("deep").join("nested").join("workflows.db");
        let host = create_host_at(&db_path).await.unwrap();
        assert!(db_path.exists());
        shutdown_host(host).await;
    }

    #[tokio::test]
    async fn test_shutdown_host_is_clean() {
        let host = create_test_host().await.unwrap();
        // Should not panic or hang
        shutdown_host(host).await;
    }

    #[tokio::test]
    async fn test_host_start_and_run_trivial_workflow() {
        let host = create_test_host().await.unwrap();

        host.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("no-op")
            .end_workflow()
            .build("test-wf", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "test-wf",
            1,
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(instance.status, WorkflowStatus::Complete);
        assert_eq!(instance.workflow_definition_id, "test-wf");
        assert_eq!(instance.execution_pointers.len(), 1);

        host.stop().await;
    }

    #[tokio::test]
    async fn test_host_multi_step_workflow() {
        let host = create_test_host().await.unwrap();

        host.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("step-a")
            .then::<NoOp>()
            .name("step-b")
            .then::<NoOp>()
            .name("step-c")
            .end_workflow()
            .build("multi-step", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "multi-step",
            1,
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(instance.status, WorkflowStatus::Complete);
        assert_eq!(instance.execution_pointers.len(), 3);

        host.stop().await;
    }

    #[tokio::test]
    async fn test_host_workflow_with_data_output() {
        let host = create_test_host().await.unwrap();

        #[derive(Default)]
        struct OutputStep;
        #[async_trait::async_trait]
        impl StepBody for OutputStep {
            async fn run(
                &mut self,
                _ctx: &StepExecutionContext<'_>,
            ) -> wfe_core::Result<ExecutionResult> {
                let mut result = ExecutionResult::next();
                result.output_data = Some(serde_json::json!({"test_key": "test_value"}));
                Ok(result)
            }
        }

        host.register_step::<OutputStep>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<OutputStep>()
            .name("output")
            .end_workflow()
            .build("output-wf", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "output-wf",
            1,
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(instance.status, WorkflowStatus::Complete);
        assert_eq!(instance.data["test_key"], "test_value");

        host.stop().await;
    }

    #[tokio::test]
    async fn test_host_get_workflow_by_id() {
        let host = create_test_host().await.unwrap();

        host.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("get-test")
            .end_workflow()
            .build("get-wf", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "get-wf",
            1,
            serde_json::json!({"initial": true}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        let fetched = host.get_workflow(&instance.id).await.unwrap();
        assert_eq!(fetched.id, instance.id);
        assert_eq!(fetched.workflow_definition_id, "get-wf");
        assert_eq!(fetched.status, WorkflowStatus::Complete);

        host.stop().await;
    }

    #[tokio::test]
    async fn test_host_with_file_sqlite_runs_workflow() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("wf-test").join("workflows.db");
        let host = create_host_at(&db_path).await.unwrap();

        host.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("file-test")
            .end_workflow()
            .build("file-wf", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "file-wf",
            1,
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(instance.status, WorkflowStatus::Complete);

        shutdown_host(host).await;
    }
}
