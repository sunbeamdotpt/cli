//! Verify workflow — VSO ↔ OpenBao end-to-end verification.

pub mod definition;
/// Steps.
pub mod steps;

use crate::info;

/// Register all verify workflow steps and the workflow definition with a host.
#[tracing::instrument(skip(host))]
pub async fn register(host: &wfe::WorkflowHost) {
    host.register_step::<steps::FindOpenBaoPod>().await;
    host.register_step::<steps::GetRootToken>().await;
    host.register_step::<steps::WriteSentinel>().await;
    host.register_step::<steps::ApplyVaultAuth>().await;
    host.register_step::<steps::ApplyVaultStaticSecret>().await;
    host.register_step::<steps::WaitForSync>().await;
    host.register_step::<steps::CheckSecretValue>().await;
    host.register_step::<steps::Cleanup>().await;
    host.register_step::<steps::PrintResult>().await;

    host.register_workflow_definition(definition::build()).await;
}

/// Print a summary of the completed verify workflow.
pub fn print_summary(logger: &crate::logger::Logger, instance: &wfe_core::models::WorkflowInstance) {
    info!(logger, "Verify workflow summary:");
    for ep in &instance.execution_pointers {
        let fallback = format!("step-{}", ep.step_id);
        let name = ep.step_name.as_deref().unwrap_or(&fallback);
        let status = format!("{:?}", ep.status);
        let duration = match (ep.start_time, ep.end_time) {
            (Some(start), Some(end)) => {
                let d = end - start;
                format!("{}ms", d.num_milliseconds())
            }
            _ => "-".to_string(),
        };
        let line = format!("  {name:<40} {status:<12} {duration}");
        info!(logger, &line);
    }
}
