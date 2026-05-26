//! Up workflow — orchestrates full cluster bring-up as composable steps.

pub mod definition;
/// Steps.
pub mod steps;

use crate::output;

/// Register all up workflow steps and the workflow definition with a host.
#[tracing::instrument(skip(host))]
pub async fn register(host: &wfe::WorkflowHost) {
    // Primitive steps (config-driven, reusable)
    host.register_step::<crate::workflows::primitives::ApplyManifest>()
        .await;
    host.register_step::<crate::workflows::primitives::WaitForRollout>()
        .await;
    host.register_step::<crate::workflows::primitives::CreatePGRole>()
        .await;
    host.register_step::<crate::workflows::primitives::CreatePGDatabase>()
        .await;
    host.register_step::<crate::workflows::primitives::EnsureNamespace>()
        .await;
    host.register_step::<crate::workflows::primitives::CreateK8sSecret>()
        .await;
    host.register_step::<crate::workflows::primitives::EnableVaultAuth>()
        .await;
    host.register_step::<crate::workflows::primitives::WriteVaultAuthConfig>()
        .await;
    host.register_step::<crate::workflows::primitives::WriteVaultPolicy>()
        .await;
    host.register_step::<crate::workflows::primitives::WriteVaultRole>()
        .await;
    host.register_step::<crate::workflows::primitives::SeedKVPath>()
        .await;
    host.register_step::<crate::workflows::primitives::WriteKVPath>()
        .await;
    host.register_step::<crate::workflows::primitives::CollectCredentials>()
        .await;
    host.register_step::<crate::workflows::primitives::EnsureOpenSearchML>()
        .await;
    host.register_step::<crate::workflows::primitives::InjectOpenSearchModelId>()
        .await;

    // Steps unique to up
    host.register_step::<steps::EnsureLimaVm>().await;
    host.register_step::<steps::EnsureCilium>().await;
    host.register_step::<steps::EnsureBuildKit>().await;
    host.register_step::<steps::BootstrapCriticalImages>().await;
    host.register_step::<steps::EnsureSeaweedFSBuckets>().await;
    host.register_step::<steps::EnsureTLSCert>().await;
    host.register_step::<steps::EnsureTLSSecret>().await;
    host.register_step::<steps::WaitForCertManagerWebhook>()
        .await;
    host.register_step::<steps::WaitForCNPGWebhook>().await;
    host.register_step::<steps::WaitForLonghornWebhook>().await;
    host.register_step::<steps::BuildProjectImages>().await;
    host.register_step::<steps::MintVpnPreAuthKeys>().await;
    host.register_step::<steps::PrintURLs>().await;

    // Steps shared from the common steps pool
    host.register_step::<steps::FindOpenBaoPod>().await;
    host.register_step::<steps::WaitPodRunning>().await;
    host.register_step::<steps::InitOrUnsealOpenBao>().await;
    host.register_step::<steps::WaitForPostgres>().await;
    host.register_step::<steps::ConfigureDatabaseEngine>().await;
    host.register_step::<steps::SeedKratosAdminIdentity>().await;

    // Register workflow definition
    host.register_workflow_definition(definition::build()).await;
}

/// Print a summary of the completed up workflow.
pub fn print_summary(instance: &wfe_core::models::WorkflowInstance) {
    tracing::info!("Up workflow summary:");
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
        tracing::info!("  {name:<40} {status:<12} {duration}");
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
        assert!(def.steps.len() > 20);
        assert_eq!(def.id, "up");

        host.stop().await;
    }

    #[tokio::test]
    async fn test_print_summary_with_missing_step_names() {
        let mut instance =
            wfe_core::models::WorkflowInstance::new("test", 1, serde_json::json!({}));
        let mut ep = wfe_core::models::ExecutionPointer::new(0);
        ep.step_name = None;
        ep.status = wfe_core::models::PointerStatus::Complete;
        ep.start_time = Some(chrono::Utc::now());
        ep.end_time = Some(chrono::Utc::now());
        instance.execution_pointers.push(ep);
        print_summary(&instance);
    }

    #[tokio::test]
    async fn test_print_summary_with_missing_times() {
        let mut instance =
            wfe_core::models::WorkflowInstance::new("test", 1, serde_json::json!({}));
        let mut ep = wfe_core::models::ExecutionPointer::new(0);
        ep.step_name = Some("test-step".to_string());
        ep.status = wfe_core::models::PointerStatus::Complete;
        ep.start_time = None;
        ep.end_time = None;
        instance.execution_pointers.push(ep);
        print_summary(&instance);
    }
}
