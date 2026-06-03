//! SeedKVPath — atomic step that seeds a single OpenBao KV path for one service.
//!
//! Creates its own port-forward, calls `get_or_create` for one service,
//! outputs per-service creds and dirty flag.

use std::collections::HashSet;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::openbao::BaoClient;

use crate::secrets::{
    self, SMTP_URI, gen_dkim_key_pair, gen_fernet_key, rand_string_32, rand_token, rand_token_n,
    scw_config,
};

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Resolve a generator function from a string type name.
/// Returns a closure that produces the secret value.
fn make_generator(gen_type: &str) -> Box<dyn Fn() -> String + Send + Sync> {
    if let Some(val) = gen_type.strip_prefix("static:") {
        let val = val.to_string();
        return Box::new(move || val.clone());
    }
    match gen_type {
        "rand_token" => Box::new(rand_token),
        "rand_token_50" => Box::new(|| rand_token_n(50)),
        "rand_string_32" => Box::new(rand_string_32),
        "fernet_key" => Box::new(gen_fernet_key),
        "scw_config_access" => Box::new(|| scw_config("access-key")),
        "scw_config_secret" => Box::new(|| scw_config("secret-key")),
        "smtp_uri" => Box::new(|| SMTP_URI.to_string()),
        "socks_proxy" => Box::new(|| format!("sunbeam:{}", rand_token())),
        _ => Box::new(String::new),
    }
}

/// Seed a single service's KV path in OpenBao.
///
/// **step_config:**
/// ```json
/// {
///   "service": "hydra",
///   "fields": [
///     {"key": "system-secret", "generator": "rand_token"},
///     {"key": "cookie-secret", "generator": "rand_token"}
///   ]
/// }
/// ```
///
/// Generator types: `rand_token`, `rand_token_50`, `fernet_key`, `smtp_uri`,
/// `scw_config_access`, `scw_config_secret`, `socks_proxy`,
/// `static:<value>`, `dkim_private`, `dkim_public`.
///
/// For `messages` service with DKIM: use gen types `dkim_private` and `dkim_public`.
/// The step will read existing DKIM keys from OpenBao and only generate if missing.
///
/// **Output:** `{"creds_{service}": {...}, "kv_data_{service}": "{...}", "dirty_{service}": true/false}`
///
/// Reads `skip_seed`, `ob_pod`, `root_token` from workflow data.
/// For `from_creds:KEY` generators, reads `creds_seaweedfs.KEY` from workflow data (requires
/// seaweedfs to be seeded first — run in a sequential branch before this step).
#[derive(Default)]
pub struct SeedKVPath;

#[async_trait::async_trait]
impl StepBody for SeedKVPath {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            tracing::info!(msg = "Skipping KV seed (skip_seed).");
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

        let config = ctx
            .step
            .step_config
            .as_ref()
            .ok_or_else(|| step_err("SeedKVPath: missing step_config"))?;
        let service = config
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| step_err("SeedKVPath: missing service"))?;
        tracing::info!(msg = "Seeding KV path...", service = %service);
        let fields = config
            .get("fields")
            .and_then(|v| v.as_array())
            .ok_or_else(|| step_err("SeedKVPath: missing fields"))?;

        let pf = secrets::port_forward("data", ob_pod, 8200)
            .await
            .map_err(|e| step_err(e.to_string()))?;
        let bao = BaoClient::with_token(&format!("http://127.0.0.1:{}", pf.local_port), root_token);

        // Handle DKIM special case: read existing keys before get_or_create
        let mut dkim_private = String::new();
        let mut dkim_public = String::new();
        let has_dkim = fields.iter().any(|f| {
            f.get("generator")
                .and_then(|g| g.as_str())
                .is_some_and(|g| g == "dkim_private" || g == "dkim_public")
        });
        if has_dkim {
            let existing = bao
                .kv_get("secret", service)
                .await
                .map_err(|e| step_err(e.to_string()))?
                .unwrap_or_default();
            if existing
                .get("dkim-private-key")
                .filter(|v| !v.is_empty())
                .is_some()
            {
                dkim_private = existing
                    .get("dkim-private-key")
                    .cloned()
                    .unwrap_or_default();
                dkim_public = existing.get("dkim-public-key").cloned().unwrap_or_default();
            } else {
                let (priv_key, pub_key) = gen_dkim_key_pair();
                dkim_private = priv_key;
                dkim_public = pub_key;
            }
        }

        // Build field generators, resolving from_creds references from workflow data
        let generators: Vec<(&str, Box<dyn Fn() -> String + Send + Sync>)> = fields
            .iter()
            .filter_map(|f| {
                let key = f.get("key")?.as_str()?;
                let gen_type = f.get("generator")?.as_str()?;

                let genfn: Box<dyn Fn() -> String + Send + Sync> =
                    if let Some(cred_key) = gen_type.strip_prefix("from_creds:") {
                        // Read from another service's output in workflow data
                        let source_service = cred_key.split('.').next().unwrap_or("");
                        let source_field = cred_key.split('.').nth(1).unwrap_or(cred_key);
                        let val = data
                            .get(format!("creds_{source_service}"))
                            .and_then(|v| v.get(source_field))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Box::new(move || val.clone())
                    } else if gen_type == "dkim_private" {
                        let v = dkim_private.clone();
                        Box::new(move || v.clone())
                    } else if gen_type == "dkim_public" {
                        let v = dkim_public.clone();
                        Box::new(move || v.clone())
                    } else {
                        make_generator(gen_type)
                    };

                Some((key, genfn))
            })
            .collect();

        let gen_refs: Vec<(&str, &(dyn Fn() -> String + Send + Sync))> =
            generators.iter().map(|(k, g)| (*k, g.as_ref())).collect();

        let mut dirty_paths: HashSet<String> = HashSet::new();
        let result_map = secrets::get_or_create(&bao, service, &gen_refs, &mut dirty_paths)
            .await
            .map_err(|e| step_err(format!("SeedKVPath({service}): {e}")))?;

        let is_dirty = dirty_paths.contains(service);
        let kv_json = serde_json::to_string(&result_map).map_err(|e| step_err(e.to_string()))?;

        tracing::info!(
            "KV seed: {service}{}",
            if is_dirty { " (new)" } else { "" }
        );

        let mut output = serde_json::Map::new();
        output.insert(
            format!("creds_{service}"),
            serde_json::to_value(&result_map).unwrap_or_default(),
        );
        output.insert(
            format!("kv_data_{service}"),
            serde_json::Value::String(kv_json),
        );
        output.insert(
            format!("dirty_{service}"),
            serde_json::Value::Bool(is_dirty),
        );

        let mut exec_result = ExecutionResult::next();
        exec_result.output_data = Some(serde_json::Value::Object(output));
        Ok(exec_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_kv_path_is_default() {
        let _ = SeedKVPath;
    }

    #[test]
    fn make_generator_rand_token() {
        let genfn = make_generator("rand_token");
        let val = genfn();
        assert!(!val.is_empty());
    }

    #[test]
    fn make_generator_static() {
        let genfn = make_generator("static:hello");
        assert_eq!(genfn(), "hello");
    }

    #[test]
    fn make_generator_empty_static() {
        let genfn = make_generator("static:");
        assert_eq!(genfn(), "");
    }

    #[test]
    fn make_generator_fernet() {
        let genfn = make_generator("fernet_key");
        let val = genfn();
        assert!(!val.is_empty());
    }

    #[test]
    fn make_generator_unknown_returns_empty() {
        let genfn = make_generator("unknown");
        assert_eq!(genfn(), "");
    }
}
