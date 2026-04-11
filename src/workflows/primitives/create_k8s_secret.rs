//! CreateK8sSecret — atomic step that creates a single Kubernetes secret.
//!
//! Reads creds from workflow data and maps them to secret keys via step_config.

use std::collections::HashMap;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::ok;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Create a single Kubernetes Opaque secret (idempotent via SSA).
///
/// **step_config:**
/// ```json
/// {
///   "namespace": "ory",
///   "name": "hydra",
///   "data": {
///     "secretsSystem": "hydra-system-secret",
///     "secretsCookie": "hydra-cookie-secret"
///   }
/// }
/// ```
///
/// Each value in `data` is a key into the `creds` map in workflow data.
/// If a value starts with `literal:`, the rest is used as the literal value.
/// If a value is `s3_json`, builds the seaweedfs s3.json blob from s3 creds.
///
/// Reads `creds` and `skip_seed` from workflow data.
#[derive(Default)]
pub struct CreateK8sSecret;

#[async_trait::async_trait]
impl StepBody for CreateK8sSecret {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(ExecutionResult::next());
        }

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("CreateK8sSecret: missing step_config"))?;
        let namespace = config
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("CreateK8sSecret: missing namespace"))?;
        let name = config
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("CreateK8sSecret: missing name"))?;
        let data_map = config
            .get("data")
            .and_then(|v| v.as_object())
            .ok_or_else(|| step_err("CreateK8sSecret: missing data"))?;

        let creds: HashMap<String, String> = data
            .get("creds")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut secret_data: HashMap<String, String> = HashMap::new();

        for (secret_key, cred_ref) in data_map {
            let cred_ref = cred_ref.as_str().unwrap_or("");
            let value = if cred_ref.starts_with("literal:") {
                cred_ref.strip_prefix("literal:").unwrap_or("").to_string()
            } else if cred_ref == "s3_json" {
                let ak = creds.get("s3-access-key").cloned().unwrap_or_default();
                let sk = creds.get("s3-secret-key").cloned().unwrap_or_default();
                crate::workflows::seed::steps::k8s_secrets::build_s3_json(&ak, &sk)
            } else {
                creds.get(cred_ref).cloned().unwrap_or_default()
            };
            secret_data.insert(secret_key.clone(), value);
        }

        k::create_secret(namespace, name, secret_data)
            .await
            .map_err(|e| step_err(format!("CreateK8sSecret({namespace}/{name}): {e}")))?;

        ok(&format!("K8s secret: {namespace}/{name}"));
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_k8s_secret_is_default() {
        let _ = CreateK8sSecret::default();
    }
}
