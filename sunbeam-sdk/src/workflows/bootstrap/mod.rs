//! Bootstrap workflow — Gitea admin setup, org creation, OIDC configuration.

pub mod definition;
/// Steps.
pub mod steps;

use crate::output;

/// Register all bootstrap workflow steps and the workflow definition with a host.
pub async fn register(host: &wfe::WorkflowHost) {
    host.register_step::<steps::GetAdminPassword>().await;
    host.register_step::<steps::WaitForGiteaPod>().await;
    host.register_step::<steps::SetAdminPassword>().await;
    host.register_step::<steps::MarkAdminPrivate>().await;
    host.register_step::<steps::CreateOrgs>().await;
    host.register_step::<steps::ConfigureOIDC>().await;
    host.register_step::<steps::PrintBootstrapResult>().await;

    host.register_workflow_definition(definition::build()).await;
}

/// Print a summary of the completed bootstrap workflow.
pub fn print_summary(instance: &wfe_core::models::WorkflowInstance) {
    output::step("Bootstrap workflow summary:");
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
        output::ok(&format!("  {name:<40} {status:<12} {duration}"));
    }
}
