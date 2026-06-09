//! WriteKVPath — atomic step that writes a single KV path to OpenBao.
//!
//! Only writes if the service was marked dirty by SeedKVPath.

use std::collections::HashMap;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::openbao::BaoClient;

use crate::secrets;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Write a single KV path to OpenBao if it was marked dirty.
///
/// **step_config:** `{"service": "hydra"}`
///
/// Reads `dirty_{service}`, `kv_data_{service}`, `ob_pod`, `root_token`,
/// `skip_seed` from workflow data.
#[derive(Default)]
pub struct WriteKVPath;

#[async_trait::async_trait]
impl StepBody for WriteKVPath {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            tracing::info!(msg = "Skipping KV write (skip_seed).");
            return Ok(ExecutionResult::next());
        }

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("WriteKVPath: missing step_config"))?;
        let service = config
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("WriteKVPath: missing service"))?;
        tracing::info!(msg = "Writing KV path...", service = %service);

        let dirty_key = format!("dirty_{service}");
        let is_dirty = data
            .get(&dirty_key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !is_dirty {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match data.get("ob_pod").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ExecutionResult::next()),
        };
        let root_token = match data.get("root_token").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return Ok(ExecutionResult::next()),
        };

        let kv_key = format!("kv_data_{service}");
        let kv_json = data.get(&kv_key).and_then(|v| v.as_str()).unwrap_or("{}");
        let path_data: HashMap<String, String> = serde_json::from_str(kv_json)
            .map_err(|e| step_err(format!("WriteKVPath({service}): bad kv_data: {e}")))?;

        let pf = secrets::port_forward("openbao", ob_pod, 8200)
            .await
            .map_err(|e| step_err(e.to_string()))?;
        let bao = BaoClient::with_token(&format!("http://127.0.0.1:{}", pf.local_port), root_token);

        bao.kv_put("secret", service, &path_data)
            .await
            .map_err(|e| step_err(format!("WriteKVPath({service}): {e}")))?;

        tracing::info!("KV write: {service}");
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_kv_path_is_default() {
        let _ = WriteKVPath;
    }
}
