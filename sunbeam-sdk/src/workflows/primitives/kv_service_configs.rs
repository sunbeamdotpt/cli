//! KV service configuration data — defines what each service needs seeded.
//!
//! Used by workflow definitions to generate SeedKVPath + WriteKVPath parallel branches.

use serde_json::{Value, json};

/// Returns the step_config for each service's SeedKVPath step.
/// Order matters: seaweedfs must come before kratos-admin (dependency).
pub fn all_service_configs() -> Vec<Value> {
    vec![
        json!({"service":"hydra","fields":[
            {"key":"system-secret","generator":"rand_token"},
            {"key":"cookie-secret","generator":"rand_token"},
            {"key":"pairwise-salt","generator":"rand_token"}
        ]}),
        json!({"service":"kratos","fields":[
            {"key":"secrets-default","generator":"rand_token"},
            {"key":"secrets-cookie","generator":"rand_token"},
            {"key":"secrets-cipher","generator":"rand_string_32"},
            {"key":"smtp-connection-uri","generator":"smtp_uri"}
        ]}),
        json!({"service":"seaweedfs","fields":[
            {"key":"access-key","generator":"rand_token"},
            {"key":"secret-key","generator":"rand_token"}
        ]}),
        json!({"service":"livekit","fields":[
            {"key":"api-key","generator":"static:devkey"},
            {"key":"api-secret","generator":"rand_token"}
        ]}),
        json!({"service":"login-ui","fields":[
            {"key":"cookie-secret","generator":"rand_token"},
            {"key":"csrf-cookie-secret","generator":"rand_token"}
        ]}),
        json!({"service":"tuwunel","fields":[
            {"key":"oidc-client-id","generator":"static:"},
            {"key":"oidc-client-secret","generator":"static:"},
            {"key":"turn-secret","generator":"static:"},
            {"key":"registration-token","generator":"rand_token"}
        ]}),
        json!({"service":"grafana","fields":[
            {"key":"admin-password","generator":"rand_token"}
        ]}),
        json!({"service":"scaleway-s3","fields":[
            {"key":"access-key-id","generator":"scw_config_access"},
            {"key":"secret-access-key","generator":"scw_config_secret"}
        ]}),
        // Headscale API key — generated *out of band* via
        // `kubectl exec -n vpn deploy/headscale -- headscale apikeys create`
        // and pasted into vault. Seed leaves a placeholder slot so the
        // path exists; replace with the real key before running
        // `sunbeam vpn create-key`.
        json!({"service":"headscale","fields":[
            {"key":"api-key","generator":"static:"}
        ]}),
    ]
}

/// Returns the config for kratos-admin, which depends on seaweedfs creds.
/// Must be seeded AFTER seaweedfs in the workflow (sequential after seaweedfs branch).
pub fn kratos_admin_config() -> Value {
    json!({"service":"kratos-admin","fields":[
        {"key":"cookie-secret","generator":"rand_token"},
        {"key":"csrf-cookie-secret","generator":"rand_token"},
        {"key":"admin-identity-ids","generator":"static:"},
        {"key":"s3-access-key","generator":"from_creds:seaweedfs.access-key"},
        {"key":"s3-secret-key","generator":"from_creds:seaweedfs.secret-key"}
    ]})
}

/// All service names (for WriteKVPath branches).
pub fn all_service_names() -> Vec<&'static str> {
    vec![
        "hydra",
        "kratos",
        "seaweedfs",
        "livekit",
        "login-ui",
        "kratos-admin",
        "tuwunel",
        "grafana",
        "scaleway-s3",
        "headscale",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_configs_have_service_and_fields() {
        for cfg in all_service_configs() {
            assert!(cfg.get("service").is_some(), "missing service in {cfg}");
            assert!(
                cfg.get("fields").and_then(|f| f.as_array()).is_some(),
                "missing fields in {cfg}"
            );
        }
    }

    #[test]
    fn service_count() {
        // 9 independent + 1 kratos-admin (dependent)
        assert_eq!(all_service_configs().len(), 9);
        assert_eq!(all_service_names().len(), 10);
    }

    #[test]
    fn kratos_admin_has_from_creds() {
        let cfg = kratos_admin_config();
        let fields = cfg["fields"].as_array().unwrap();
        let s3_field = fields.iter().find(|f| f["key"] == "s3-access-key").unwrap();
        assert!(
            s3_field["generator"]
                .as_str()
                .unwrap()
                .starts_with("from_creds:")
        );
    }
}
