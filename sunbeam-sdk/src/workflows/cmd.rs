//! Workflow CLI actions — local WFE host, remote wfe-server, and target management.

use clap::Subcommand;

use crate::error::{Result, SunbeamError};
use crate::output;

use super::host;

#[derive(Subcommand, Debug)]
/// Workflow action.
pub enum WorkflowAction {
    // ------------------------------------------------------------------
    // Commands that work on both local and remote targets
    // ------------------------------------------------------------------
    /// List workflow instances.
    List {
        /// Filter by status (runnable, complete, terminated, suspended).
        #[arg(long, default_value = "")]
        status: String,
        /// Free-text query (remote only).
        #[arg(long)]
        query: Option<String>,
        /// Maximum results (remote only).
        #[arg(long, default_value_t = 50)]
        limit: u64,
        /// Skip N results (remote only).
        #[arg(long, default_value_t = 0)]
        skip: u64,
    },
    /// Cancel a running workflow.
    Cancel {
        /// Workflow instance ID.
        id: String,
    },

    // ------------------------------------------------------------------
    // Local-only commands
    // ------------------------------------------------------------------
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
    /// Run a YAML-defined workflow locally.
    Run {
        /// Path to workflow YAML file (default: ./workflows.yaml).
        #[arg(default_value = "")]
        file: String,
    },

    // ------------------------------------------------------------------
    // Remote-only commands (wfe-server)
    // ------------------------------------------------------------------
    /// Register a workflow definition from a YAML file.
    Register(crate::wfectl::register::RegisterArgs),
    /// Locally validate a workflow YAML file (no server round-trip).
    Validate(crate::wfectl::validate::ValidateArgs),
    /// Manage registered workflow definitions.
    Definitions(crate::wfectl::definitions::DefinitionsArgs),
    /// Start a registered workflow instance on the server.
    #[command(name = "start")]
    Start(crate::wfectl::run::RunArgs),
    /// Get a workflow instance by ID or name.
    Get(crate::wfectl::get::GetArgs),
    /// Suspend a running workflow.
    Suspend(crate::wfectl::suspend::SuspendArgs),
    /// Resume a suspended workflow.
    Resume(crate::wfectl::resume::ResumeArgs),
    /// Publish an event to waiting workflows.
    Publish(crate::wfectl::publish::PublishArgs),
    /// Stream lifecycle events.
    Watch(crate::wfectl::watch::WatchArgs),
    /// Stream step logs.
    Logs(crate::wfectl::logs::LogsArgs),
    /// Full-text search log lines.
    SearchLogs(crate::wfectl::search_logs::SearchLogsArgs),

    // ------------------------------------------------------------------
    // Target management
    // ------------------------------------------------------------------
    /// Authenticate with and save a remote workflow server target.
    Login {
        /// Target name.
        #[arg(short, long)]
        name: String,
        /// Server URL (e.g. https://builds.sunbeam.pt).
        #[arg(short, long)]
        url: String,
    },
    /// Remove a saved target.
    Logout {
        /// Target name.
        #[arg(short, long)]
        name: String,
    },
    /// List saved workflow targets.
    Targets,
}

/// Resolve the effective target name and optional config.
fn resolve_target(target: Option<&str>) -> Result<(String, Option<crate::config::WorkflowTarget>)> {
    let cfg = crate::config::load_config();

    let name = target
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .or_else(|| {
            if cfg.default_workflow_target.is_empty() {
                None
            } else {
                Some(cfg.default_workflow_target.clone())
            }
        })
        .unwrap_or_else(|| "local".to_string());

    if name == "local" {
        return Ok((name, None));
    }

    match cfg.workflow_targets.get(&name) {
        Some(t) => Ok((name, Some(t.clone()))),
        None => Err(SunbeamError::Config(format!(
            "workflow target '{name}' not found — run `sunbeam workflow login --name {name} --url <url>`"
        ))),
    }
}

/// Dispatch a `sunbeam workflow <action>` command.
#[tracing::instrument(skip(action), fields(target = tracing::field::Empty))]
pub async fn dispatch(
    target: Option<&str>,
    action: WorkflowAction,
    output: crate::wfectl::output::OutputFormat,
) -> Result<()> {
    let (target_name, target_cfg) = resolve_target(target)?;
    tracing::Span::current().record("target", &target_name);

    // Target management commands work regardless of target.
    match action {
        WorkflowAction::Login { name, url } => {
            return login_target(&name, &url);
        }
        WorkflowAction::Logout { name } => {
            return logout_target(&name);
        }
        WorkflowAction::Targets => {
            return list_targets();
        }
        _ => {}
    }

    if target_name == "local" {
        dispatch_local(action).await
    } else {
        let t = target_cfg.expect("remote target resolved");
        dispatch_remote(action, output, &t).await
    }
}

// ---------------------------------------------------------------------------
// Local dispatch
// ---------------------------------------------------------------------------

async fn dispatch_local(action: WorkflowAction) -> Result<()> {
    match action {
        WorkflowAction::List { status, .. } => {
            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = list_workflows(&h, &status).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::Status { id } => {
            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = show_workflow_status(&h, &id).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::Retry { id } => {
            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = retry_workflow(&h, &id).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::Cancel { id } => {
            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = cancel_workflow(&h, &id).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::Run { file } => run_workflow(&file).await,
        _ => Err(SunbeamError::Other(format!(
            "command '{action:?}' is not supported for local target — use a remote target with `-t <name>`"
        ))),
    }
}

/// Inner dispatch that operates on an already-created host. Testable.
#[tracing::instrument(skip(h))]
pub async fn dispatch_with_host(h: &wfe::WorkflowHost, action: WorkflowAction) -> Result<()> {
    match action {
        WorkflowAction::List { status, .. } => list_workflows(h, &status).await,
        WorkflowAction::Status { id } => show_workflow_status(h, &id).await,
        WorkflowAction::Retry { id } => retry_workflow(h, &id).await,
        WorkflowAction::Cancel { id } => cancel_workflow(h, &id).await,
        WorkflowAction::Run { .. } => unreachable!("handled above"),
        _ => Err(SunbeamError::Other(format!(
            "command '{action:?}' is not supported for local target"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Remote dispatch
// ---------------------------------------------------------------------------

async fn dispatch_remote(
    action: WorkflowAction,
    output: crate::wfectl::output::OutputFormat,
    target: &crate::config::WorkflowTarget,
) -> Result<()> {
    use crate::wfectl::output::OutputFormat;

    // Derive domain from URL for token resolution.
    let domain = extract_domain(&target.url)?;

    // Validate is the only command that doesn't need a server connection.
    if let WorkflowAction::Validate(args) = action {
        return crate::wfectl::validate::run(args, output)
            .await
            .map_err(|e| SunbeamError::Other(format!("{e:#}")));
    }

    let token = crate::wfectl::resolve_token(&domain)
        .map_err(|e| SunbeamError::Other(format!("{e:#}")))?;
    let client = crate::wfectl::client::build(&target.url, &token)
        .await
        .map_err(|e| SunbeamError::Other(format!("{e:#}")))?;

    let result = match action {
        WorkflowAction::List { query, status, limit, skip } => {
            let args = crate::wfectl::list::ListArgs {
                query,
                status: parse_status_filter(&status),
                limit,
                skip,
            };
            crate::wfectl::list::run(args, client, output).await
        }
        WorkflowAction::Cancel { id } => {
            let args = crate::wfectl::cancel::CancelArgs { workflow_id: id };
            crate::wfectl::cancel::run(args, client).await
        }
        WorkflowAction::Register(args) => {
            crate::wfectl::register::run(args, client, output).await
        }
        WorkflowAction::Definitions(args) => {
            crate::wfectl::definitions::run(args, client, output).await
        }
        WorkflowAction::Start(args) => {
            crate::wfectl::run::run(args, client, output).await
        }
        WorkflowAction::Get(args) => {
            crate::wfectl::get::run(args, client, output).await
        }
        WorkflowAction::Suspend(args) => {
            crate::wfectl::suspend::run(args, client).await
        }
        WorkflowAction::Resume(args) => {
            crate::wfectl::resume::run(args, client).await
        }
        WorkflowAction::Publish(args) => {
            crate::wfectl::publish::run(args, client, output).await
        }
        WorkflowAction::Watch(args) => {
            crate::wfectl::watch::run(args, client).await
        }
        WorkflowAction::Logs(args) => {
            crate::wfectl::logs::run(args, client).await
        }
        WorkflowAction::SearchLogs(args) => {
            crate::wfectl::search_logs::run(args, client, output).await
        }
        WorkflowAction::Validate(_) => unreachable!(),
        _ => {
            return Err(SunbeamError::Other(format!(
                "command '{action:?}' is not supported for remote target — use `-t local`"
            )));
        }
    };

    result.map_err(|e| SunbeamError::Other(format!("{e:#}")))
}

fn extract_domain(url: &str) -> Result<String> {
    // Strip scheme if present.
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    // Strip path/port if present.
    let host = rest.split('/').next().unwrap_or(rest);
    Ok(host.to_string())
}

fn parse_status_filter(status: &str) -> Option<crate::wfectl::list::StatusFilter> {
    match status.to_lowercase().as_str() {
        "runnable" => Some(crate::wfectl::list::StatusFilter::Runnable),
        "suspended" => Some(crate::wfectl::list::StatusFilter::Suspended),
        "complete" => Some(crate::wfectl::list::StatusFilter::Complete),
        "terminated" => Some(crate::wfectl::list::StatusFilter::Terminated),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Target management
// ---------------------------------------------------------------------------

fn login_target(name: &str, url: &str) -> Result<()> {
    let mut cfg = crate::config::load_config();
    cfg.workflow_targets.insert(
        name.to_string(),
        crate::config::WorkflowTarget {
            url: url.to_string(),
            token: String::new(),
        },
    );
    crate::config::save_config(&cfg)?;
    tracing::info!("saved workflow target '{name}' -> {url}");
    Ok(())
}

fn logout_target(name: &str) -> Result<()> {
    let mut cfg = crate::config::load_config();
    if cfg.workflow_targets.remove(name).is_some() {
        crate::config::save_config(&cfg)?;
        tracing::info!("removed workflow target '{name}'");
    } else {
        tracing::warn!("workflow target '{name}' not found");
    }
    Ok(())
}

fn target_row(r: &TargetRow) -> Vec<String> {
    vec![r.name.clone(), r.url.clone()]
}

#[derive(serde::Serialize)]
struct TargetRow {
    name: String,
    url: String,
}

fn list_targets() -> Result<()> {
    let cfg = crate::config::load_config();

    let mut rows = vec![TargetRow {
        name: "local".to_string(),
        url: "(implicit)".to_string(),
    }];
    for (name, target) in &cfg.workflow_targets {
        rows.push(TargetRow {
            name: name.clone(),
            url: target.url.clone(),
        });
    }

    output::render_list(&rows, &["NAME", "URL"], target_row, crate::output::OutputFormat::Table)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Local command implementations
// ---------------------------------------------------------------------------

/// List workflow instances.
#[tracing::instrument(skip(h))]
pub async fn list_workflows(h: &wfe::WorkflowHost, _status_filter: &str) -> Result<()> {
    let now = chrono::Utc::now();
    let ids = h
        .persistence()
        .get_runnable_instances(now)
        .await
        .map_err(|e| SunbeamError::Other(format!("query workflows: {e}")))?;

    if ids.is_empty() {
        tracing::info!("No workflow instances found.");
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
#[tracing::instrument(skip(h))]
pub async fn show_workflow_status(h: &wfe::WorkflowHost, id: &str) -> Result<()> {
    match h.get_workflow(id).await {
        Ok(wf) => {
            tracing::info!("Workflow: {}", wf.workflow_definition_id);
            tracing::info!("Status:   {:?}", wf.status);
            tracing::info!("Created:  {}", wf.create_time);
            if let Some(ct) = wf.complete_time {
                tracing::info!("Completed: {ct}");
            }

            println!();
            tracing::info!("Execution pointers:");
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
            tracing::warn!("Workflow instance '{id}' not found: {e}");
        }
    }

    Ok(())
}

/// Resume a suspended/failed workflow.
#[tracing::instrument(skip(h))]
pub async fn retry_workflow(h: &wfe::WorkflowHost, id: &str) -> Result<()> {
    h.resume_workflow(id)
        .await
        .map_err(|e| SunbeamError::Other(format!("resume workflow: {e}")))?;
    tracing::info!("Workflow '{id}' resumed.");
    Ok(())
}

/// Terminate a running workflow.
#[tracing::instrument(skip(h))]
pub async fn cancel_workflow(h: &wfe::WorkflowHost, id: &str) -> Result<()> {
    h.terminate_workflow(id)
        .await
        .map_err(|e| SunbeamError::Other(format!("terminate workflow: {e}")))?;
    tracing::info!("Workflow '{id}' cancelled.");
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
                query: None,
                limit: 50,
                skip: 0,
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
