//! K8s secret mirroring steps: sync Gitea admin password.
//!
//! CreateK8sSecrets is now handled by the CreateK8sSecret primitive.

use std::collections::HashMap;

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::{ok, warn};
use crate::secrets::GITEA_ADMIN_USER;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

fn json_bool(data: &serde_json::Value, key: &str) -> bool {
    data.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

// -- Pure helpers (used by CreateK8sSecret primitive) -------------------------

fn get_cred(creds: &HashMap<String, String>, key: &str) -> String {
    creds.get(key).cloned().unwrap_or_default()
}

pub(crate) fn build_s3_json(access_key: &str, secret_key: &str) -> String {
    serde_json::json!({
        "identities": [{
            "name": "seaweed",
            "credentials": [{"accessKey": access_key, "secretKey": secret_key}],
            "actions": ["Admin", "Read", "Write", "List", "Tagging"]
        }]
    }).to_string()
}

// -- SyncGiteaAdminPassword -------------------------------------------------

/// Sync gitea admin password to Gitea's own DB.
///
/// Reads: `skip_seed`, `creds`
#[derive(Default)]
pub struct SyncGiteaAdminPassword;

#[async_trait::async_trait]
impl StepBody for SyncGiteaAdminPassword {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if json_bool(data, "skip_seed") {
            return Ok(ExecutionResult::next());
        }

        let creds: HashMap<String, String> = data.get("creds")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let gitea_admin_pass = get_cred(&creds, "gitea-admin-password");
        if gitea_admin_pass.is_empty() {
            return Ok(ExecutionResult::next());
        }

        let client = k::get_client().await.map_err(|e| step_err(e.to_string()))?;
        let gitea_pods: Api<Pod> = Api::namespaced(client.clone(), "devtools");
        let lp = ListParams::default().labels("app.kubernetes.io/name=gitea");
        if let Ok(pod_list) = gitea_pods.list(&lp).await {
            if let Some(gitea_pod) = pod_list.items.first().and_then(|p| p.metadata.name.as_deref()) {
                match k::kube_exec(
                    "devtools", gitea_pod,
                    &[
                        "gitea", "admin", "user", "change-password",
                        "--username", GITEA_ADMIN_USER,
                        "--password", &gitea_admin_pass,
                        "--must-change-password=false",
                    ],
                    Some("gitea"),
                ).await {
                    Ok((0, _)) => ok("Gitea admin password synced to Gitea DB."),
                    Ok((_, stderr)) => warn(&format!("Could not sync Gitea admin password: {stderr}")),
                    Err(e) => warn(&format!("Could not sync Gitea admin password: {e}")),
                }
            } else {
                warn("Gitea pod not found -- admin password NOT synced.");
            }
        }

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::WorkflowStatus;

    async fn run_step<S: StepBody + Default + 'static>(
        data: serde_json::Value,
    ) -> wfe_core::models::WorkflowInstance {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        host.register_step::<S>().await;
        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<S>()
            .name("test-step")
            .end_workflow()
            .build("test-wf", 1);
        host.register_workflow_definition(def).await;
        let instance = run_workflow_sync(&host, "test-wf", 1, data, Duration::from_secs(5))
            .await
            .unwrap();
        host.stop().await;
        instance
    }

    #[tokio::test]
    async fn test_sync_gitea_admin_password_skip_seed() {
        let instance = run_step::<SyncGiteaAdminPassword>(serde_json::json!({ "skip_seed": true })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_sync_gitea_admin_password_empty() {
        let instance = run_step::<SyncGiteaAdminPassword>(serde_json::json!({ "skip_seed": false })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[test]
    fn test_build_s3_json_valid() {
        let json = build_s3_json("AK", "SK");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["identities"][0]["credentials"][0]["accessKey"], "AK");
    }
}
