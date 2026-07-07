//! Workflow CLI actions — local WFE host, remote wfe-server, and target management.

use clap::Subcommand;

use crate::output;
use sunbeam_sdk::error::{Result, SunbeamError};
use sunbeam_sdk::{error, info};

use sunbeam_sdk::workflows::host;

#[derive(Subcommand, Debug)]
/// Workflow action.
pub enum WorkflowAction {
    /// Cancel a running workflow.
    #[command(long_about = r#"Cancel a running or suspended workflow instance.

Sends a termination signal. The workflow stops at its next checkpoint.
Already-completed steps are not rolled back.

EXAMPLE:
  sunbeam workflow cancel <instance-id>
"#)]
    Cancel {
        /// Workflow instance ID.
        id: String,
    },
    /// Manage registered workflow definitions.
    #[command(long_about = r#"List or manage registered workflow definitions.

Shows all definitions available on the remote server with their versions
and step counts.

EXAMPLE:
  sunbeam workflow -t builds definitions list
"#)]
    Definitions(sunbeam_sdk::wfectl::definitions::DefinitionsArgs),
    /// Get a workflow instance by ID or name.
    #[command(long_about = r#"Get workflow instance details from the remote server.

Similar to `status` but queries the server-side state rather than local SQLite.

EXAMPLE:
  sunbeam workflow -t builds get <instance-id>
"#)]
    Get(sunbeam_sdk::wfectl::get::GetArgs),
    /// List workflow instances.
    #[command(long_about = r#"List workflow instances.

Local target: queries the SQLite database for runnable instances.
Remote target: queries the wfe-server with optional status filter, pagination,
and full-text query.

EXAMPLES:
  sunbeam workflow list
  sunbeam workflow list --status complete
  sunbeam workflow -t builds list --limit 100
"#)]
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
    /// Authenticate with and save a remote workflow server target.
    #[command(long_about = r#"Register a remote workflow server target.

Saves the server URL in ~/.sunbeam/config.json. Authentication tokens are
resolved via `sunbeam auth token` using the domain derived from the URL.

EXAMPLE:
  sunbeam workflow login --name builds --url https://builds.sunbeam.pt
"#)]
    Login {
        /// Target name.
        #[arg(short, long)]
        name: String,
        /// Server URL (e.g. https://builds.sunbeam.pt).
        #[arg(short, long)]
        url: String,
    },
    /// Remove a saved target.
    #[command(long_about = r#"Remove a saved remote target.

Deletes the target from ~/.sunbeam/config.json. Does not revoke tokens.

EXAMPLE:
  sunbeam workflow logout --name builds
"#)]
    Logout {
        /// Target name.
        #[arg(short, long)]
        name: String,
    },
    /// Stream step logs.
    #[command(long_about = r#"Stream step execution logs.

Tails the structured logs emitted by a specific workflow instance's steps.
Useful for debugging long-running workflows.

EXAMPLE:
  sunbeam workflow -t builds logs <instance-id>
"#)]
    Logs(sunbeam_sdk::wfectl::logs::LogsArgs),
    /// Publish an event to waiting workflows.
    #[command(long_about = r#"Publish an event to waiting workflows.

Workflows that are blocked on `wait_for_event` will consume this event
and continue.

EXAMPLE:
  sunbeam workflow -t builds publish --event deploy-complete --key prod
"#)]
    Publish(sunbeam_sdk::wfectl::publish::PublishArgs),
    /// Register a workflow definition from a YAML file.
    #[command(long_about = r#"Register a workflow definition on a remote server.

Uploads a YAML workflow definition to the wfe-server so it can be started
by name and version later.

EXAMPLE:
  sunbeam workflow -t builds register ./deploy.yaml
"#)]
    Register(sunbeam_sdk::wfectl::register::RegisterArgs),
    /// Resume a suspended workflow.
    #[command(long_about = r#"Resume a suspended workflow instance.

Continues execution from the last checkpoint.

EXAMPLE:
  sunbeam workflow -t builds resume <instance-id>
"#)]
    Resume(sunbeam_sdk::wfectl::resume::ResumeArgs),
    /// Retry a failed workflow from its last checkpoint.
    #[command(long_about = r#"Resume a failed or suspended workflow.

Retries from the last successful checkpoint. Steps that already completed
are skipped. Useful after fixing an underlying issue (e.g. a pod that was
stuck in Pending).

EXAMPLE:
  sunbeam workflow retry <instance-id>
"#)]
    Retry {
        /// Workflow instance ID.
        id: String,
    },
    /// Run a YAML-defined workflow locally.
    #[command(long_about = r#"Run a workflow from a YAML file.

Not yet implemented for local target. Use remote target (-t) with the
`start` command instead.

EXAMPLE:
  sunbeam workflow run ./deploy.yaml
"#)]
    Run {
        /// Path to workflow YAML file (default: ./workflows.yaml).
        #[arg(default_value = "")]
        file: String,
    },
    /// Full-text search log lines.
    #[command(long_about = r#"Search workflow logs.

Performs full-text search across stored step logs on the remote server.

EXAMPLE:
  sunbeam workflow -t builds search-logs <instance-id> --query "error"
"#)]
    SearchLogs(sunbeam_sdk::wfectl::search_logs::SearchLogsArgs),
    /// Start a registered workflow instance on the server.
    #[command(
        name = "start",
        long_about = r#"Start a registered workflow on the remote server.

Creates a new workflow instance from a previously registered definition.
Returns the instance ID for tracking.

EXAMPLE:
  sunbeam workflow -t builds start --definition deploy --version 1
"#
    )]
    Start(sunbeam_sdk::wfectl::run::RunArgs),
    /// Show status of a workflow instance.
    #[command(long_about = r#"Show detailed status of a workflow instance.

Prints the workflow definition, overall status, creation/completion times,
and a table of every step with its status, start/end times, and retry count.

EXAMPLE:
  sunbeam workflow status <instance-id>
"#)]
    Status {
        /// Workflow instance ID.
        id: String,
    },
    /// Suspend a running workflow.
    #[command(long_about = r#"Suspend a running workflow instance.

Pauses execution at the next checkpoint. Can be resumed with `resume`.

EXAMPLE:
  sunbeam workflow -t builds suspend <instance-id>
"#)]
    Suspend(sunbeam_sdk::wfectl::suspend::SuspendArgs),
    /// List saved workflow targets.
    #[command(long_about = r#"List all saved workflow targets.

Shows name and URL for each registered remote target. The implicit `local`
target is always listed.

EXAMPLE:
  sunbeam workflow targets
"#)]
    Targets,
    /// Locally validate a workflow YAML file (no server round-trip).
    #[command(long_about = r#"Validate a workflow YAML file locally.

Checks syntax, step references, and wiring without connecting to a server.
Useful in CI before registering.

EXAMPLE:
  sunbeam workflow validate ./deploy.yaml
"#)]
    Validate(sunbeam_sdk::wfectl::validate::ValidateArgs),
    /// Stream lifecycle events.
    #[command(long_about = r#"Stream workflow lifecycle events.

Connects to the server's SSE endpoint and prints workflow start, complete,
step transition, and failure events in real time.

EXAMPLE:
  sunbeam workflow -t builds watch
"#)]
    Watch(sunbeam_sdk::wfectl::watch::WatchArgs),
}

/// Resolve the effective target name and optional config.
fn resolve_target(
    target: Option<&str>,
) -> Result<(String, Option<sunbeam_sdk::config::WorkflowTarget>)> {
    let cfg = sunbeam_sdk::config::load_config();

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
    output: sunbeam_sdk::wfectl::output::OutputFormat,
) -> Result<()> {
    let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
    let (target_name, target_cfg) = resolve_target(target)?;
    tracing::Span::current().record("target", &target_name);

    // Target management commands work regardless of target.
    match action {
        WorkflowAction::Login { name, url } => {
            return login_target(&logger, &name, &url);
        }
        WorkflowAction::Logout { name } => {
            return logout_target(&logger, &name);
        }
        WorkflowAction::Targets => {
            return list_targets();
        }
        _ => {}
    }

    if target_name == "local" {
        dispatch_local(&logger, action).await
    } else {
        let t = match target_cfg {
            Some(t) => t,
            // resolve_target returns Some for any non-local target that exists.
            None => unreachable!(),
        };
        dispatch_remote(&logger, action, output, &t).await
    }
}

// ---------------------------------------------------------------------------
// Local dispatch
// ---------------------------------------------------------------------------

async fn dispatch_local(
    logger: &sunbeam_sdk::logger::Logger,
    action: WorkflowAction,
) -> Result<()> {
    match action {
        WorkflowAction::Cancel { id } => {
            let ctx_name = {
                let cfg = sunbeam_sdk::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = cancel_workflow(logger, &h, &id).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::List { status, .. } => {
            let ctx_name = {
                let cfg = sunbeam_sdk::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = list_workflows(logger, &h, &status).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::Retry { id } => {
            let ctx_name = {
                let cfg = sunbeam_sdk::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = retry_workflow(logger, &h, &id).await;
            host::shutdown_host(h).await;
            result
        }
        WorkflowAction::Run { file } => run_workflow(&file).await,
        WorkflowAction::Status { id } => {
            let ctx_name = {
                let cfg = sunbeam_sdk::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            let h = host::create_host(&ctx_name).await?;
            let result = show_workflow_status(logger, &h, &id).await;
            host::shutdown_host(h).await;
            result
        }
        _ => Err(SunbeamError::Other(format!(
            "command '{action:?}' is not supported for local target — use a remote target with `-t <name>`"
        ))),
    }
}

/// Inner dispatch that operates on an already-created host. Testable.
#[cfg(test)]
#[tracing::instrument(skip(h, logger))]
pub async fn dispatch_with_host(
    logger: &sunbeam_sdk::logger::Logger,
    h: &wfe::WorkflowHost,
    action: WorkflowAction,
) -> Result<()> {
    match action {
        WorkflowAction::Cancel { id } => cancel_workflow(logger, h, &id).await,
        WorkflowAction::List { status, .. } => list_workflows(logger, h, &status).await,
        WorkflowAction::Retry { id } => retry_workflow(logger, h, &id).await,
        WorkflowAction::Run { .. } => unreachable!("handled above"),
        WorkflowAction::Status { id } => show_workflow_status(logger, h, &id).await,
        _ => Err(SunbeamError::Other(format!(
            "command '{action:?}' is not supported for local target"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Remote dispatch
// ---------------------------------------------------------------------------

async fn dispatch_remote(
    logger: &sunbeam_sdk::logger::Logger,
    action: WorkflowAction,
    output: sunbeam_sdk::wfectl::output::OutputFormat,
    target: &sunbeam_sdk::config::WorkflowTarget,
) -> Result<()> {
    // Derive domain from URL for token resolution.
    let domain = extract_domain(&target.url)?;

    // Validate is the only command that doesn't need a server connection.
    if let WorkflowAction::Validate(args) = action {
        return sunbeam_sdk::wfectl::validate::run(args, output)
            .await
            .map_err(|e| SunbeamError::Other(format!("{e:#}")));
    }

    let token = sunbeam_sdk::wfectl::resolve_token(&domain)
        .map_err(|e| SunbeamError::Other(format!("{e:#}")))?;
    let client = sunbeam_sdk::wfectl::client::build(logger, &target.url, &token)
        .await
        .map_err(|e| SunbeamError::Other(format!("{e:#}")))?;

    let result = match action {
        WorkflowAction::Cancel { id } => {
            let args = sunbeam_sdk::wfectl::cancel::CancelArgs { workflow_id: id };
            sunbeam_sdk::wfectl::cancel::run(logger, args, client).await
        }
        WorkflowAction::Definitions(args) => {
            sunbeam_sdk::wfectl::definitions::run(args, client, output).await
        }
        WorkflowAction::Get(args) => {
            sunbeam_sdk::wfectl::get::run(logger, args, client, output).await
        }
        WorkflowAction::List {
            query,
            status,
            limit,
            skip,
        } => {
            let args = sunbeam_sdk::wfectl::list::ListArgs {
                query,
                status: parse_status_filter(&status),
                limit,
                skip,
            };
            sunbeam_sdk::wfectl::list::run(logger, args, client, output).await
        }
        WorkflowAction::Logs(args) => sunbeam_sdk::wfectl::logs::run(logger, args, client).await,
        WorkflowAction::Publish(args) => {
            sunbeam_sdk::wfectl::publish::run(args, client, output).await
        }
        WorkflowAction::Register(args) => {
            sunbeam_sdk::wfectl::register::run(args, client, output).await
        }
        WorkflowAction::Resume(args) => {
            sunbeam_sdk::wfectl::resume::run(logger, args, client).await
        }
        WorkflowAction::SearchLogs(args) => {
            sunbeam_sdk::wfectl::search_logs::run(args, client, output).await
        }
        WorkflowAction::Start(args) => {
            sunbeam_sdk::wfectl::run::run(logger, args, client, output).await
        }
        WorkflowAction::Suspend(args) => {
            sunbeam_sdk::wfectl::suspend::run(logger, args, client).await
        }
        WorkflowAction::Validate(_) => unreachable!(),
        WorkflowAction::Watch(args) => sunbeam_sdk::wfectl::watch::run(args, client).await,
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
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // Strip path/port if present.
    let host = rest.split('/').next().unwrap_or(rest);
    Ok(host.to_string())
}

fn parse_status_filter(status: &str) -> Option<sunbeam_sdk::wfectl::list::StatusFilter> {
    match status.to_lowercase().as_str() {
        "runnable" => Some(sunbeam_sdk::wfectl::list::StatusFilter::Runnable),
        "suspended" => Some(sunbeam_sdk::wfectl::list::StatusFilter::Suspended),
        "complete" => Some(sunbeam_sdk::wfectl::list::StatusFilter::Complete),
        "terminated" => Some(sunbeam_sdk::wfectl::list::StatusFilter::Terminated),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Target management
// ---------------------------------------------------------------------------

fn login_target(logger: &sunbeam_sdk::logger::Logger, name: &str, url: &str) -> Result<()> {
    let mut cfg = sunbeam_sdk::config::load_config();
    cfg.workflow_targets.insert(
        name.to_string(),
        sunbeam_sdk::config::WorkflowTarget {
            url: url.to_string(),
        },
    );
    sunbeam_sdk::config::save_config(&cfg)?;
    info!(logger, "saved workflow target", name = name, url = url);
    Ok(())
}

fn logout_target(logger: &sunbeam_sdk::logger::Logger, name: &str) -> Result<()> {
    let mut cfg = sunbeam_sdk::config::load_config();
    if cfg.workflow_targets.remove(name).is_some() {
        sunbeam_sdk::config::save_config(&cfg)?;
        info!(logger, "removed workflow target", name = name);
    } else {
        info!(logger, "workflow target not found", name = name);
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
    let cfg = sunbeam_sdk::config::load_config();

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

    output::render_list(
        &rows,
        &["NAME", "URL"],
        target_row,
        output::OutputFormat::Table,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Local command implementations
// ---------------------------------------------------------------------------

/// List workflow instances.
#[tracing::instrument(skip(h, logger))]
pub async fn list_workflows(
    logger: &sunbeam_sdk::logger::Logger,
    h: &wfe::WorkflowHost,
    _status_filter: &str,
) -> Result<()> {
    let now = chrono::Utc::now();
    let ids = h
        .persistence()
        .get_runnable_instances(now)
        .await
        .map_err(|e| SunbeamError::Other(format!("query workflows: {e}")))?;

    if ids.is_empty() {
        info!(logger, "No workflow instances found.");
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
#[tracing::instrument(skip(h, logger))]
pub async fn show_workflow_status(
    logger: &sunbeam_sdk::logger::Logger,
    h: &wfe::WorkflowHost,
    id: &str,
) -> Result<()> {
    match h.get_workflow(id).await {
        Ok(wf) => {
            info!(
                logger,
                "Workflow:",
                definition_id = wf.workflow_definition_id
            );
            info!(logger, "Status:", status = format!("{:?}", wf.status));
            info!(logger, "Created:", create_time = wf.create_time);
            if let Some(ct) = wf.complete_time {
                info!(logger, "Completed:", completed = ct);
            }

            println!();
            info!(logger, "Execution pointers:");
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
            error!(logger, "Workflow instance not found", id = id, err = e);
        }
    }

    Ok(())
}

/// Resume a suspended/failed workflow.
#[tracing::instrument(skip(h, logger))]
pub async fn retry_workflow(
    logger: &sunbeam_sdk::logger::Logger,
    h: &wfe::WorkflowHost,
    id: &str,
) -> Result<()> {
    h.resume_workflow(id)
        .await
        .map_err(|e| SunbeamError::Other(format!("resume workflow: {e}")))?;
    info!(logger, "Workflow resumed.", id = id);
    Ok(())
}

/// Terminate a running workflow.
#[tracing::instrument(skip(h, logger))]
pub async fn cancel_workflow(
    logger: &sunbeam_sdk::logger::Logger,
    h: &wfe::WorkflowHost,
    id: &str,
) -> Result<()> {
    h.terminate_workflow(id)
        .await
        .map_err(|e| SunbeamError::Other(format!("terminate workflow: {e}")))?;
    info!(logger, "Workflow cancelled.", id = id);
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
    use async_trait::async_trait;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::{ExecutionResult, WorkflowInstance, WorkflowStatus};
    use wfe_core::traits::{StepBody, StepExecutionContext};

    #[derive(Default)]
    struct NoOp;
    #[async_trait]
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
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();
        let result = list_workflows(&logger, &h, "").await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_show_workflow_status_not_found() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();
        let result = show_workflow_status(&logger, &h, "nonexistent-id").await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_show_workflow_status_found() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let (h, id) = setup_host_with_workflow().await;
        let result = show_workflow_status(&logger, &h, &id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_show_status_with_step_details() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
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
        let result = show_workflow_status(&logger, &h, &instance.id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_cancel_workflow_completed() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let (h, id) = setup_host_with_workflow().await;
        let result = cancel_workflow(&logger, &h, &id).await;
        drop(result);
        h.stop().await;
    }

    #[tokio::test]
    async fn test_retry_workflow_nonexistent() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();
        let result = retry_workflow(&logger, &h, "does-not-exist").await;
        assert!(result.is_err());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_cancel_workflow_nonexistent() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();
        let result = cancel_workflow(&logger, &h, "does-not-exist").await;
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
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();
        let result = dispatch_with_host(
            &logger,
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
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let (h, id) = setup_host_with_workflow().await;
        let result = dispatch_with_host(&logger, &h, WorkflowAction::Status { id }).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_dispatch_with_host_retry_nonexistent() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();
        let result = dispatch_with_host(
            &logger,
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
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
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
        let result = retry_workflow(&logger, &h, &id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_cancel_running_workflow() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
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

        let result = cancel_workflow(&logger, &h, &id).await;
        assert!(result.is_ok());
        h.stop().await;
    }

    #[tokio::test]
    async fn test_dispatch_with_host_cancel() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let (h, id) = setup_host_with_workflow().await;
        let result = dispatch_with_host(&logger, &h, WorkflowAction::Cancel { id }).await;
        drop(result);
        h.stop().await;
    }

    #[tokio::test]
    async fn test_list_workflows_with_runnable_instance() {
        let logger = sunbeam_sdk::logger::Logger::new(sunbeam_sdk::logger::TracingSink);
        let h = host::create_test_host().await.unwrap();

        // Manually persist a Runnable workflow so get_runnable_instances finds it
        let instance = WorkflowInstance::new("manual-def", 1, serde_json::json!({}));

        h.persistence()
            .create_new_workflow(&instance)
            .await
            .unwrap();

        // Now list_workflows should hit the non-empty path
        let result = list_workflows(&logger, &h, "").await;
        assert!(result.is_ok());
        h.stop().await;
    }
}
