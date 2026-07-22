//! `wfectl cancel <workflow-id>` -- cancel a running workflow.

use anyhow::Result;
use clap::Args;
use wfe_server_protos::wfe::v1::CancelWorkflowRequest;

use super::client::AuthClient;
use sdk::info;

#[derive(Debug, Args)]
/// Cancelargs.
pub struct CancelArgs {
    /// Workflow instance identifier — UUID or human-friendly name (e.g. "ci-42").
    pub workflow_id: String,
}

/// Run.
#[tracing::instrument(skip(logger))]
pub async fn run(
    logger: &sdk::logger::Logger,
    args: CancelArgs,
    mut client: AuthClient,
) -> Result<()> {
    info!(logger, "wfectl cancel workflow", id = args.workflow_id);
    client
        .cancel_workflow(CancelWorkflowRequest {
            workflow_id: args.workflow_id.clone(),
        })
        .await?;
    println!("✓ Cancelled workflow {}", args.workflow_id);
    Ok(())
}
