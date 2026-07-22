//! Down workflow — orchestrates cluster tear-down as composable WFE steps.

pub mod definition;
/// Steps.
pub mod steps;

use sdk::info;

/// Register all down workflow steps and the workflow definition with a host.
#[tracing::instrument(skip(host))]
pub async fn register(host: &wfe::WorkflowHost) {
    host.register_step::<steps::DiscoverNamespaces>().await;
    host.register_step::<steps::DeleteNamespaces>().await;
    host.register_step::<steps::WaitForTermination>().await;
    host.register_step::<steps::ForceDeleteStuckNamespaces>()
        .await;

    host.register_workflow_definition(definition::build()).await;
}

/// Print a summary of the completed down workflow.
pub fn print_summary(logger: &sdk::logger::Logger, instance: &wfe_core::models::WorkflowInstance) {
    info!(logger, "Down workflow summary:");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_all_steps_and_definition() {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        register(&host).await;

        let def = definition::build();
        assert!(!def.steps.is_empty());
        assert_eq!(def.id, "down");

        host.stop().await;
    }
}
