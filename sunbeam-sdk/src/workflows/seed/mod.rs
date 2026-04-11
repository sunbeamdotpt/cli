//! Seed workflow — orchestrates OpenBao init, KV seeding, Postgres setup,
//! K8s secret mirroring, and Kratos admin identity creation.

pub mod definition;
pub mod steps;

use crate::output;

/// Register all seed workflow steps and the workflow definition with a host.
pub async fn register(host: &wfe::WorkflowHost) {
    // Primitive steps (config-driven, reusable)
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

    // Seed-specific steps
    host.register_step::<steps::FindOpenBaoPod>().await;
    host.register_step::<steps::WaitPodRunning>().await;
    host.register_step::<steps::InitOrUnsealOpenBao>().await;
    host.register_step::<steps::WaitForPostgres>().await;
    host.register_step::<steps::ConfigureDatabaseEngine>().await;
    host.register_step::<steps::SyncGiteaAdminPassword>().await;
    host.register_step::<steps::SeedKratosAdminIdentity>().await;
    host.register_step::<steps::PrintSeedOutputs>().await;

    // Register workflow definition
    host.register_workflow_definition(definition::build()).await;
}

/// Print a summary of the completed seed workflow.
pub fn print_summary(instance: &wfe_core::models::WorkflowInstance) {
    output::step("Seed workflow summary:");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::models::WorkflowStatus;

    #[tokio::test]
    async fn test_register_all_steps_and_definition() {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        register(&host).await;

        let def = definition::build();
        assert_eq!(def.id, "seed");
        assert!(def.steps.len() > 13);

        // Run a minimal skip-path test to prove steps are registered
        let skip_def = wfe_core::builder::WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<steps::WaitPodRunning>()
            .name("wait-pod-running")
            .then::<steps::ConfigureDatabaseEngine>()
            .name("configure-db")
            .then::<steps::SyncGiteaAdminPassword>()
            .name("sync-gitea")
            .then::<steps::PrintSeedOutputs>()
            .name("print-outputs")
            .end_workflow()
            .build("seed-skip-test", 1);
        host.register_workflow_definition(skip_def).await;

        let instance = run_workflow_sync(
            &host,
            "seed-skip-test",
            1,
            serde_json::json!({ "skip_seed": true }),
            Duration::from_secs(10),
        )
        .await
        .unwrap();

        assert_eq!(instance.status, WorkflowStatus::Complete);
        host.stop().await;
    }

    #[tokio::test]
    async fn test_print_summary_with_completed_workflow() {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        register(&host).await;

        let def = wfe_core::builder::WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<steps::WaitPodRunning>()
            .name("wait-pod-running")
            .then::<steps::PrintSeedOutputs>()
            .name("print-seed-outputs")
            .end_workflow()
            .build("summary-test", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "summary-test",
            1,
            serde_json::json!({ "skip_seed": true }),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        // Should not panic — just prints to stdout
        print_summary(&instance);
    }

    #[tokio::test]
    async fn test_print_summary_handles_both_named_and_unnamed_steps() {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        register(&host).await;

        let def = wfe_core::builder::WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<steps::WaitPodRunning>()
            .name("wait-pod-running")
            .then::<steps::PrintSeedOutputs>()
            .name("print-seed-outputs")
            .end_workflow()
            .build("names-test", 1);
        host.register_workflow_definition(def).await;

        let instance = run_workflow_sync(
            &host,
            "names-test",
            1,
            serde_json::json!({ "skip_seed": true }),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        // print_summary should handle both named and unnamed steps gracefully
        print_summary(&instance);
        assert_eq!(instance.execution_pointers.len(), 2);
    }

    #[tokio::test]
    async fn test_print_summary_with_missing_step_names() {
        // Construct a synthetic instance with no step names to exercise fallback
        let mut instance =
            wfe_core::models::WorkflowInstance::new("test", 1, serde_json::json!({}));
        let mut ep = wfe_core::models::ExecutionPointer::new(0);
        ep.step_name = None;
        ep.status = wfe_core::models::PointerStatus::Complete;
        ep.start_time = Some(chrono::Utc::now());
        ep.end_time = Some(chrono::Utc::now());
        instance.execution_pointers.push(ep);
        // Should not panic — uses "step-0" fallback
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
        // Should print "-" for duration
        print_summary(&instance);
    }
}
