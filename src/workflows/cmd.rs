use clap::Subcommand;
use wfe_core::traits::WorkflowRepository;

use crate::error::{Result, SunbeamError};
use crate::output;

use super::host;

#[derive(Subcommand, Debug)]
pub enum WorkflowAction {
    /// List workflow instances.
    List {
        /// Filter by status (runnable, complete, terminated, suspended).
        #[arg(long, default_value = "")]
        status: String,
    },
    /// Show status of a workflow instance.
    Status {
        /// Workflow instance ID.
        id: String,
    },
    /// Retry a failed workflow from its last checkpoint.
    Retry {
        /// Workflow instance ID.
        id: String,
    },
    /// Cancel a running workflow.
    Cancel {
        /// Workflow instance ID.
        id: String,
    },
    /// Run a YAML-defined workflow.
    Run {
        /// Path to workflow YAML file (default: ./workflows.yaml).
        #[arg(default_value = "")]
        file: String,
    },
}

/// Dispatch a `sunbeam workflow <action>` command.
pub async fn dispatch(context_name: &str, action: WorkflowAction) -> Result<()> {
    if let WorkflowAction::Run { file } = action {
        return run_workflow(&file).await;
    }

    let h = host::create_host(context_name).await?;
    let result = dispatch_with_host(&h, action).await;
    host::shutdown_host(h).await;
    result
}

/// Inner dispatch that operates on an already-created host. Testable.
pub async fn dispatch_with_host(h: &wfe::WorkflowHost, action: WorkflowAction) -> Result<()> {
    match action {
        WorkflowAction::List { status } => list_workflows(h, &status).await,
        WorkflowAction::Status { id } => show_workflow_status(h, &id).await,
        WorkflowAction::Retry { id } => retry_workflow(h, &id).await,
        WorkflowAction::Cancel { id } => cancel_workflow(h, &id).await,
        WorkflowAction::Run { .. } => unreachable!("handled above"),
    }
}

/// List workflow instances.
pub async fn list_workflows(h: &wfe::WorkflowHost, _status_filter: &str) -> Result<()> {
    let now = chrono::Utc::now();
    let ids = h
        .persistence()
        .get_runnable_instances(now)
        .await
        .map_err(|e| SunbeamError::Other(format!("query workflows: {e}")))?;

    if ids.is_empty() {
        output::ok("No workflow instances found.");
        return Ok(());
    }

    let instances = h
        .persistence()
        .get_workflow_instances(&ids)
        .await
        .map_err(|e| SunbeamError::Other(format!("load workflows: {e}")))?;

    let rows: Vec<Vec<String>> = instances
        .iter()
        .map(|wf| {
            vec![
                wf.id.clone(),
                wf.workflow_definition_id.clone(),
                format!("{:?}", wf.status),
            ]
        })
        .collect();

    println!("{}", output::table(&rows, &["ID", "DEFINITION", "STATUS"]));
    Ok(())
}

/// Show status of a single workflow instance.
pub async fn show_workflow_status(h: &wfe::WorkflowHost, id: &str) -> Result<()> {
    match h.get_workflow(id).await {
        Ok(wf) => {
            output::ok(&format!("Workflow: {}", wf.workflow_definition_id));
            output::ok(&format!("Status:   {:?}", wf.status));
            output::ok(&format!("Created:  {}", wf.create_time));
            if let Some(ct) = wf.complete_time {
                output::ok(&format!("Completed: {ct}"));
            }

            println!();
            output::step("Execution pointers:");
            let rows: Vec<Vec<String>> = wf
                .execution_pointers
                .iter()
                .map(|ep| {
                    vec![
                        ep.step_name
                            .clone()
                            .unwrap_or_else(|| format!("step-{}", ep.step_id)),
                        format!("{:?}", ep.status),
                        ep.start_time.map(|t| t.to_string()).unwrap_or_default(),
                        ep.end_time.map(|t| t.to_string()).unwrap_or_default(),
                        format!("{}", ep.retry_count),
                    ]
                })
                .collect();

            println!(
                "{}",
                output::table(&rows, &["STEP", "STATUS", "STARTED", "ENDED", "RETRIES"])
            );
        }
        Err(e) => {
            output::warn(&format!("Workflow instance '{id}' not found: {e}"));
        }
    }

    Ok(())
}

/// Resume a suspended/failed workflow.
pub async fn retry_workflow(h: &wfe::WorkflowHost, id: &str) -> Result<()> {
    h.resume_workflow(id)
        .await
        .map_err(|e| SunbeamError::Other(format!("resume workflow: {e}")))?;
    output::ok(&format!("Workflow '{id}' resumed."));
    Ok(())
}

/// Terminate a running workflow.
pub async fn cancel_workflow(h: &wfe::WorkflowHost, id: &str) -> Result<()> {
    h.terminate_workflow(id)
        .await
        .map_err(|e| SunbeamError::Other(format!("terminate workflow: {e}")))?;
    output::ok(&format!("Workflow '{id}' cancelled."));
    Ok(())
}

async fn run_workflow(_file: &str) -> Result<()> {
    Err(SunbeamError::Other(
        "sunbeam workflow run is not yet implemented".to_string(),
    ))
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

    async fn setup_host_with_workflow() -> (wfe::WorkflowHost, String) {
        let h = host::create_test_host().await.unwrap();
        h.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("test-step")
            .end_workflow()
            .build("test-def", 1);
        h.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &h,
            "test-def",
            1,
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        (h, instance.id)
    }

    #[tokio::test]
    async fn test_list_workflows_empty() {
        let h = host::create_test_host().await.unwrap();
        let result = list_workflows(&h, "").await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_show_workflow_status_not_found() {
        let h = host::create_test_host().await.unwrap();
        let result = show_workflow_status(&h, "nonexistent-id").await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_show_workflow_status_found() {
        let (h, id) = setup_host_with_workflow().await;
        let result = show_workflow_status(&h, &id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_show_status_with_step_details() {
        let h = host::create_test_host().await.unwrap();
        h.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("step-alpha")
            .then::<NoOp>()
            .name("step-beta")
            .end_workflow()
            .build("multi-def", 1);
        h.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &h,
            "multi-def",
            1,
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(instance.status, WorkflowStatus::Complete);
        let result = show_workflow_status(&h, &instance.id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_cancel_workflow_completed() {
        let (h, id) = setup_host_with_workflow().await;
        let result = cancel_workflow(&h, &id).await;
        drop(result);
        h.stop().await;
    }

    #[tokio::test]
    async fn test_retry_workflow_nonexistent() {
        let h = host::create_test_host().await.unwrap();
        let result = retry_workflow(&h, "does-not-exist").await;
        assert!(result.is_err());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_cancel_workflow_nonexistent() {
        let h = host::create_test_host().await.unwrap();
        let result = cancel_workflow(&h, "does-not-exist").await;
        assert!(result.is_err());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_run_workflow_not_implemented() {
        let result = run_workflow("test.yaml").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not yet implemented")
        );
    }

    #[tokio::test]
    async fn test_dispatch_with_host_list() {
        let h = host::create_test_host().await.unwrap();
        let result = dispatch_with_host(
            &h,
            WorkflowAction::List {
                status: String::new(),
            },
        )
        .await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_dispatch_with_host_status() {
        let (h, id) = setup_host_with_workflow().await;
        let result = dispatch_with_host(&h, WorkflowAction::Status { id }).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_dispatch_with_host_retry_nonexistent() {
        let h = host::create_test_host().await.unwrap();
        let result = dispatch_with_host(
            &h,
            WorkflowAction::Retry {
                id: "nope".to_string(),
            },
        )
        .await;
        assert!(result.is_err());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_retry_suspended_workflow() {
        let h = host::create_test_host().await.unwrap();
        h.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("suspend-step")
            .end_workflow()
            .build("suspend-def", 1);
        h.register_workflow_definition(def).await;

        let id = h
            .start_workflow("suspend-def", 1, serde_json::json!({}))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Suspend it
        let _ = h.suspend_workflow(&id).await;

        // Resume should succeed
        let result = retry_workflow(&h, &id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_cancel_running_workflow() {
        let h = host::create_test_host().await.unwrap();
        h.register_step::<NoOp>().await;

        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<NoOp>()
            .name("cancel-step")
            .wait_for("never-event", "never-key")
            .name("waiting")
            .end_workflow()
            .build("cancel-def", 1);
        h.register_workflow_definition(def).await;

        let id = h
            .start_workflow("cancel-def", 1, serde_json::json!({}))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let result = cancel_workflow(&h, &id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_dispatch_with_host_cancel() {
        let (h, id) = setup_host_with_workflow().await;
        let result = dispatch_with_host(&h, WorkflowAction::Cancel { id }).await;
        drop(result);
        h.stop().await;
    }

    #[tokio::test]
    async fn test_list_workflows_with_runnable_instance() {
        use wfe_core::models::WorkflowInstance;

        let h = host::create_test_host().await.unwrap();

        // Manually persist a Runnable workflow so get_runnable_instances finds it
        let instance = WorkflowInstance::new("manual-def", 1, serde_json::json!({}));

        h.persistence()
            .create_new_workflow(&instance)
            .await
            .unwrap();

        // Now list_workflows should hit the non-empty path
        let result = list_workflows(&h, "").await;
        assert!(result.is_ok());
        h.stop().await;
    }
}
