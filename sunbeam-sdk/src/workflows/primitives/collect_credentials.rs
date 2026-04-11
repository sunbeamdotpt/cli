//! CollectCredentials — joins per-service cred outputs into a unified `creds` map.
//!
//! Runs after all SeedKVPath steps complete (parallel join point).

use std::collections::HashMap;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::output::ok;

/// Credential mapping: maps a global cred key to a service + field.
/// E.g., `("hydra-system-secret", "hydra", "system-secret")` means
/// `creds["hydra-system-secret"] = creds_hydra["system-secret"]`.
const CRED_MAPPINGS: &[(&str, &str, &str)] = &[
    ("hydra-system-secret", "hydra", "system-secret"),
    ("hydra-cookie-secret", "hydra", "cookie-secret"),
    ("hydra-pairwise-salt", "hydra", "pairwise-salt"),
    ("kratos-secrets-default", "kratos", "secrets-default"),
    ("kratos-secrets-cookie", "kratos", "secrets-cookie"),
    ("s3-access-key", "seaweedfs", "access-key"),
    ("s3-secret-key", "seaweedfs", "secret-key"),
    ("gitea-admin-password", "gitea", "admin-password"),
    ("hive-oidc-client-id", "hive", "oidc-client-id"),
    ("hive-oidc-client-secret", "hive", "oidc-client-secret"),
    ("people-django-secret", "people", "django-secret-key"),
    ("livekit-api-key", "livekit", "api-key"),
    ("livekit-api-secret", "livekit", "api-secret"),
    (
        "kratos-admin-cookie-secret",
        "kratos-admin",
        "cookie-secret",
    ),
    ("messages-dkim-public-key", "messages", "dkim-public-key"),
];

/// All services that have KV data to collect.
const KV_SERVICES: &[&str] = &[
    "hydra",
    "kratos",
    "seaweedfs",
    "gitea",
    "hive",
    "livekit",
    "people",
    "login-ui",
    "kratos-admin",
    "docs",
    "meet",
    "drive",
    "projects",
    "calendars",
    "messages",
    "collabora",
    "tuwunel",
    "grafana",
    "scaleway-s3",
];

/// Collect per-service credential outputs into a unified `creds` map.
///
/// Reads `creds_{service}` and `kv_data_{service}` for each service from workflow data.
/// Outputs `creds` (unified HashMap) and `dirty_paths` (Vec of dirty service names).
#[derive(Default)]
pub struct CollectCredentials;

#[async_trait::async_trait]
impl StepBody for CollectCredentials {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if data
            .get("skip_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Ok(ExecutionResult::next());
        }

        let mut creds: HashMap<String, String> = HashMap::new();
        let mut dirty_paths: Vec<String> = Vec::new();

        // Collect per-service creds into the global map
        for (global_key, service, field) in CRED_MAPPINGS {
            let value = data
                .get(format!("creds_{service}"))
                .and_then(|v| v.get(*field))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            creds.insert(global_key.to_string(), value);
        }

        // Collect kv_data and dirty flags
        for service in KV_SERVICES {
            let kv_key = format!("kv_data_{service}");
            if let Some(kv_json) = data.get(&kv_key).and_then(|v| v.as_str()) {
                creds.insert(format!("kv_data/{service}"), kv_json.to_string());
            }

            let dirty_key = format!("dirty_{service}");
            if data
                .get(&dirty_key)
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                dirty_paths.push(service.to_string());
            }
        }

        ok(&format!(
            "Collected credentials from {} services ({} dirty)",
            KV_SERVICES.len(),
            dirty_paths.len()
        ));

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({
            "creds": creds,
            "dirty_paths": dirty_paths,
        }));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_credentials_is_default() {
        let _ = CollectCredentials;
    }

    #[test]
    fn cred_mappings_cover_all_expected_keys() {
        assert_eq!(CRED_MAPPINGS.len(), 15);
        assert!(
            CRED_MAPPINGS
                .iter()
                .any(|(k, _, _)| *k == "hydra-system-secret")
        );
        assert!(
            CRED_MAPPINGS
                .iter()
                .any(|(k, _, _)| *k == "messages-dkim-public-key")
        );
    }

    #[test]
    fn kv_services_count() {
        assert_eq!(KV_SERVICES.len(), 19);
    }
}
