//! `wfectl cancel <workflow-id>` -- cancel a running workflow.

use anyhow::Result;
use clap::Args;
use wfe_server_protos::wfe::v1::CancelWorkflowRequest;

use super::client::AuthClient;

#[derive(Debug, Args)]
/// Cancelargs.
pub struct CancelArgs {
    /// Workflow instance identifier — UUID or human-friendly name (e.g. "ci-42").
    pub workflow_id: String,
}

/// Run.
#[tracing::instrument]
pub async fn run(args: CancelArgs, mut client: AuthClient) -> Result<()> {
    tracing::info!("wfectl cancel workflow {id}", id = args.workflow_id);
    client
        .cancel_workflow(CancelWorkflowRequest {
            workflow_id: args.workflow_id.clone(),
        })
        .await?;
    println!("✓ Cancelled workflow {}", args.workflow_id);
    Ok(())
}
