//! `wfectl resume <workflow-id>` -- resume a suspended workflow.

use anyhow::Result;
use clap::Args;
use wfe_server_protos::wfe::v1::ResumeWorkflowRequest;

use super::client::AuthClient;
use sdk::info;

#[derive(Debug, Args)]
/// Resumeargs.
pub struct ResumeArgs {
    /// Workflow instance identifier — UUID or human-friendly name (e.g. "ci-42").
    pub workflow_id: String,
}

/// Run.
#[tracing::instrument(skip(logger))]
pub async fn run(
    logger: &sdk::logger::Logger,
    args: ResumeArgs,
    mut client: AuthClient,
) -> Result<()> {
    info!(logger, "wfectl resume workflow", id = args.workflow_id);
    client
        .resume_workflow(ResumeWorkflowRequest {
            workflow_id: args.workflow_id.clone(),
        })
        .await?;
    println!("✓ Resumed workflow {}", args.workflow_id);
    Ok(())
}
