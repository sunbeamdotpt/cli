//! Steps for the verify workflow — VSO ↔ OpenBao E2E verification.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use sdk::info;
use sdk::k8s_openapi;
use sdk::kube as k;
use sdk::kube_rs as kube;
use sdk::openbao::BaoClient;

use crate::workflows::data::VerifyData;
use sdk::secrets;

const TEST_NS: &str = "ory";
const TEST_NAME: &str = "vso-verify";

fn load_data(ctx: &StepExecutionContext<'_>) -> wfe_core::Result<VerifyData> {
    serde_json::from_value(ctx.workflow.data.clone())
        .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))
}

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

// ── FindOpenBaoPod ─────────────────────────────────────────────────────────

/// Find the OpenBao server pod by label selector.
#[derive(Default)]
pub struct FindOpenBaoPod;

#[async_trait::async_trait]
impl StepBody for FindOpenBaoPod {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("find_openbao_pod");
        let data = load_data(ctx)?;
        let step_ctx = data
            .ctx
            .as_ref()
            .ok_or_else(|| step_err("missing __ctx in workflow data"))?;

        k::set_context(&step_ctx.kube_context);

        let client = k::get_client().await.map_err(|e| step_err(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "openbao");
        let lp = kube::api::ListParams::default()
            .labels("app.kubernetes.io/name=openbao,component=server");
        let pod_list = pods.list(&lp).await.map_err(|e| step_err(e.to_string()))?;

        let ob_pod = pod_list
            .items
            .first()
            .and_then(|p| p.metadata.name.as_deref())
            .ok_or_else(|| step_err("OpenBao pod not found -- run full bring-up first"))?;

        tracing::info!("OpenBao pod: {ob_pod}");

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({ "ob_pod": ob_pod }));
        Ok(result)
    }
}

// ── GetRootToken ───────────────────────────────────────────────────────────

/// Read the root token from the openbao-bootstrap-token K8s secret.
#[derive(Default)]
pub struct GetRootToken;

#[async_trait::async_trait]
impl StepBody for GetRootToken {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("get_root_token");
        let root_token =
            k::kube_get_secret_field("openbao", "openbao-bootstrap-token", "root-token")
                .await
                .map_err(|e| {
                    step_err(format!(
                        "Could not read openbao-bootstrap-token secret: {e}"
                    ))
                })?;

        tracing::info!("Root token retrieved.");

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({ "root_token": root_token }));
        Ok(result)
    }
}

// ── WriteSentinel ──────────────────────────────────────────────────────────

/// Write a random test sentinel value to OpenBao secret/vso-test.
#[derive(Default)]
pub struct WriteSentinel;

#[async_trait::async_trait]
impl StepBody for WriteSentinel {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("write_sentinel");
        let data = load_data(ctx)?;
        let ob_pod = data
            .ob_pod
            .as_deref()
            .ok_or_else(|| step_err("ob_pod not set"))?;
        let root_token = data
            .root_token
            .as_deref()
            .ok_or_else(|| step_err("root_token not set"))?;

        let pf = secrets::port_forward("openbao", ob_pod, 8200)
            .await
            .map_err(|e| step_err(e.to_string()))?;
        let bao = BaoClient::with_token(&format!("http://127.0.0.1:{}", pf.local_port), root_token);

        let test_value = secrets::rand_token_n(16);
        tracing::info!("Writing test sentinel to OpenBao secret/vso-test...");

        let mut kv_data = std::collections::HashMap::new();
        kv_data.insert("test-key".to_string(), test_value.clone());
        bao.kv_put("secret", "vso-test", &kv_data)
            .await
            .map_err(|e| step_err(e.to_string()))?;

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({ "test_value": test_value }));
        Ok(result)
    }
}

// ── ApplyVaultAuth ─────────────────────────────────────────────────────────

/// Create the VaultAuth CRD for the test.
pub struct ApplyVaultAuth {
    logger: sdk::logger::Logger,
}

impl Default for ApplyVaultAuth {
    fn default() -> Self {
        Self {
            logger: sdk::logger::Logger::new(sdk::logger::TracingSink),
        }
    }
}

#[async_trait::async_trait]
impl StepBody for ApplyVaultAuth {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let logger = &self.logger;
        info!(
            logger,
            "Creating VaultAuth",
            namespace = TEST_NS,
            name = TEST_NAME
        );
        k::kube_apply(
            logger,
            &format!(
                r#"
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultAuth
metadata:
  name: {TEST_NAME}
  namespace: {TEST_NS}
spec:
  method: kubernetes
  mount: kubernetes
  kubernetes:
    role: vso
    serviceAccount: default
"#
            ),
        )
        .await
        .map_err(|e| step_err(e.to_string()))?;

        Ok(ExecutionResult::next())
    }
}

// ── ApplyVaultStaticSecret ─────────────────────────────────────────────────

/// Create the VaultStaticSecret CRD that VSO will sync.
pub struct ApplyVaultStaticSecret {
    logger: sdk::logger::Logger,
}

impl Default for ApplyVaultStaticSecret {
    fn default() -> Self {
        Self {
            logger: sdk::logger::Logger::new(sdk::logger::TracingSink),
        }
    }
}

#[async_trait::async_trait]
impl StepBody for ApplyVaultStaticSecret {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let logger = &self.logger;
        info!(
            logger,
            "Creating VaultStaticSecret",
            namespace = TEST_NS,
            name = TEST_NAME
        );
        k::kube_apply(
            logger,
            &format!(
                r#"
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultStaticSecret
metadata:
  name: {TEST_NAME}
  namespace: {TEST_NS}
spec:
  vaultAuthRef: {TEST_NAME}
  mount: secret
  type: kv-v2
  path: vso-test
  refreshAfter: 10s
  destination:
    name: {TEST_NAME}
    create: true
    overwrite: true
"#
            ),
        )
        .await
        .map_err(|e| step_err(e.to_string()))?;

        Ok(ExecutionResult::next())
    }
}

// ── WaitForSync ────────────────────────────────────────────────────────────

/// Wait for VSO to sync the secret (up to 60s).
#[derive(Default)]
pub struct WaitForSync;

#[async_trait::async_trait]
impl StepBody for WaitForSync {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("wait_for_sync");
        tracing::info!("Waiting for VSO to sync (up to 60s)...");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut synced = false;

        while tokio::time::Instant::now() < deadline {
            let mac = vso_status_field(TEST_NS, TEST_NAME, |status| {
                status
                    .get("secretMAC")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .await;
            if !mac.is_empty() {
                synced = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }

        if !synced {
            let msg = vso_status_field(TEST_NS, TEST_NAME, |status| {
                status
                    .get("conditions")
                    .and_then(|c| c.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .await;
            return Err(step_err(format!(
                "VSO did not sync within 60s. Last status: {}",
                if msg.is_empty() {
                    "unknown".to_string()
                } else {
                    msg
                }
            )));
        }

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({ "synced": true }));
        Ok(result)
    }
}

// ── CheckSecretValue ───────────────────────────────────────────────────────

/// Verify the K8s Secret contains the expected sentinel value.
#[derive(Default)]
pub struct CheckSecretValue;

#[async_trait::async_trait]
impl StepBody for CheckSecretValue {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("check_secret_value");
        let data = load_data(ctx)?;
        let test_value = data
            .test_value
            .as_deref()
            .ok_or_else(|| step_err("test_value not set"))?;

        tracing::info!("Verifying K8s Secret contents...");

        let secret = k::kube_get_secret(TEST_NS, TEST_NAME)
            .await
            .map_err(|e| step_err(e.to_string()))?
            .ok_or_else(|| step_err(format!("K8s Secret {TEST_NS}/{TEST_NAME} not found")))?;

        let secret_data = secret
            .data
            .as_ref()
            .ok_or_else(|| step_err("Secret has no data"))?;
        let raw = secret_data
            .get("test-key")
            .ok_or_else(|| step_err("Missing key 'test-key' in secret"))?;
        let actual =
            String::from_utf8(raw.0.clone()).map_err(|e| step_err(format!("UTF-8 error: {e}")))?;

        if actual != test_value {
            return Err(step_err(format!(
                "Value mismatch!\n  expected: {:?}\n  got:      {:?}",
                test_value, actual
            )));
        }

        tracing::info!("Sentinel value matches -- VSO -> OpenBao integration is working.");
        Ok(ExecutionResult::next())
    }
}

// ── Cleanup ────────────────────────────────────────────────────────────────

/// Clean up all test resources (always runs).
#[derive(Default)]
pub struct Cleanup;

#[async_trait::async_trait]
impl StepBody for Cleanup {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("cleanup");
        tracing::info!("Cleaning up test resources...");

        let _ = secrets::delete_resource(TEST_NS, "vaultstaticsecret", TEST_NAME).await;
        let _ = secrets::delete_resource(TEST_NS, "vaultauth", TEST_NAME).await;

        // Delete the K8s Secret
        if let Ok(client) = k::get_client().await {
            let api: kube::Api<k8s_openapi::api::core::v1::Secret> =
                kube::Api::namespaced(client.clone(), TEST_NS);
            let _ = api
                .delete(TEST_NAME, &kube::api::DeleteParams::default())
                .await;
        }

        // Delete the vault KV entry
        let data = load_data(ctx)?;
        if let (Some(ob_pod), Some(root_token)) =
            (data.ob_pod.as_deref(), data.root_token.as_deref())
            && let Ok(pf) = secrets::port_forward("openbao", ob_pod, 8200).await
        {
            let bao =
                BaoClient::with_token(&format!("http://127.0.0.1:{}", pf.local_port), root_token);
            let _ = bao.kv_delete("secret", "vso-test").await;
        }

        Ok(ExecutionResult::next())
    }
}

// ── PrintResult ────────────────────────────────────────────────────────────

/// Print final verification result.
#[derive(Default)]
pub struct PrintResult;

#[async_trait::async_trait]
impl StepBody for PrintResult {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        tracing::debug!("print_result");
        let data = load_data(ctx)?;
        if data.synced {
            tracing::info!("VSO E2E verification passed.");
        } else {
            tracing::error!("VSO verification did not complete successfully.");
        }
        Ok(ExecutionResult::next())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Fetch a VaultStaticSecret and return a value extracted from its `.status`.
/// `extract` walks the `status` JSON and returns a string (empty if missing).
async fn vso_status_field(
    ns: &str,
    name: &str,
    extract: impl Fn(&serde_json::Value) -> String,
) -> String {
    let client = match k::get_client().await {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let gvk =
        kube::api::GroupVersionKind::gvk("secrets.hashicorp.com", "v1beta1", "VaultStaticSecret");
    let ar = kube::api::ApiResource::from_gvk(&gvk);
    let api: kube::Api<kube::api::DynamicObject> =
        kube::Api::namespaced_with(client.clone(), ns, &ar);
    match api.get_opt(name).await {
        Ok(Some(obj)) => {
            let status = obj.data.get("status").cloned().unwrap_or_default();
            extract(&status)
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_openbao_pod_is_default() {
        let _ = FindOpenBaoPod;
    }

    #[test]
    fn get_root_token_is_default() {
        let _ = GetRootToken;
    }

    #[test]
    fn write_sentinel_is_default() {
        let _ = WriteSentinel;
    }

    #[test]
    fn apply_vault_auth_is_default() {
        let _ = ApplyVaultAuth::default();
    }

    #[test]
    fn apply_vault_static_secret_is_default() {
        let _ = ApplyVaultStaticSecret::default();
    }

    #[test]
    fn wait_for_sync_is_default() {
        let _ = WaitForSync;
    }

    #[test]
    fn check_secret_value_is_default() {
        let _ = CheckSecretValue;
    }

    #[test]
    fn cleanup_is_default() {
        let _ = Cleanup;
    }

    #[test]
    fn print_result_is_default() {
        let _ = PrintResult;
    }

    #[test]
    fn test_constants() {
        assert_eq!(TEST_NS, "ory");
        assert_eq!(TEST_NAME, "vso-verify");
    }
}
