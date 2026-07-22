//! `wfectl suspend <workflow-id>` -- pause a running workflow.

use anyhow::Result;
use clap::Args;
use wfe_server_protos::wfe::v1::SuspendWorkflowRequest;

use super::client::AuthClient;
use sdk::info;

#[derive(Debug, Args)]
/// Suspendargs.
pub struct SuspendArgs {
    /// Workflow instance identifier — UUID or human-friendly name (e.g. "ci-42").
    pub workflow_id: String,
}

/// Run.
#[tracing::instrument(skip(logger))]
pub async fn run(
    logger: &sdk::logger::Logger,
    args: SuspendArgs,
    mut client: AuthClient,
) -> Result<()> {
    info!(logger, "wfectl suspend workflow", id = args.workflow_id);
    client
        .suspend_workflow(SuspendWorkflowRequest {
            workflow_id: args.workflow_id.clone(),
        })
        .await?;
    println!("✓ Suspended workflow {}", args.workflow_id);
    Ok(())
}
