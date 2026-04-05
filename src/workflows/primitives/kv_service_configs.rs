//! KV service configuration data — defines what each service needs seeded.
//!
//! Used by workflow definitions to generate SeedKVPath + WriteKVPath parallel branches.

use serde_json::{json, Value};

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
            {"key":"smtp-connection-uri","generator":"smtp_uri"}
        ]}),
        json!({"service":"seaweedfs","fields":[
            {"key":"access-key","generator":"rand_token"},
            {"key":"secret-key","generator":"rand_token"}
        ]}),
        json!({"service":"gitea","fields":[
            {"key":"admin-username","generator":"gitea_admin"},
            {"key":"admin-password","generator":"rand_token"}
        ]}),
        json!({"service":"hive","fields":[
            {"key":"oidc-client-id","generator":"static:hive-local"},
            {"key":"oidc-client-secret","generator":"rand_token"}
        ]}),
        json!({"service":"livekit","fields":[
            {"key":"api-key","generator":"static:devkey"},
            {"key":"api-secret","generator":"rand_token"}
        ]}),
        json!({"service":"people","fields":[
            {"key":"django-secret-key","generator":"rand_token"}
        ]}),
        json!({"service":"login-ui","fields":[
            {"key":"cookie-secret","generator":"rand_token"},
            {"key":"csrf-cookie-secret","generator":"rand_token"}
        ]}),
        json!({"service":"docs","fields":[
            {"key":"django-secret-key","generator":"rand_token"},
            {"key":"collaboration-secret","generator":"rand_token"}
        ]}),
        json!({"service":"meet","fields":[
            {"key":"django-secret-key","generator":"rand_token"},
            {"key":"application-jwt-secret-key","generator":"rand_token"}
        ]}),
        json!({"service":"drive","fields":[
            {"key":"django-secret-key","generator":"rand_token"}
        ]}),
        json!({"service":"projects","fields":[
            {"key":"secret-key","generator":"rand_token"}
        ]}),
        json!({"service":"calendars","fields":[
            {"key":"django-secret-key","generator":"rand_token_50"},
            {"key":"salt-key","generator":"rand_token"},
            {"key":"caldav-inbound-api-key","generator":"rand_token"},
            {"key":"caldav-outbound-api-key","generator":"rand_token"},
            {"key":"caldav-internal-api-key","generator":"rand_token"}
        ]}),
        json!({"service":"messages","fields":[
            {"key":"django-secret-key","generator":"rand_token"},
            {"key":"salt-key","generator":"rand_token"},
            {"key":"mda-api-secret","generator":"rand_token"},
            {"key":"oidc-refresh-token-key","generator":"fernet_key"},
            {"key":"dkim-private-key","generator":"dkim_private"},
            {"key":"dkim-public-key","generator":"dkim_public"},
            {"key":"rspamd-password","generator":"rand_token"},
            {"key":"socks-proxy-users","generator":"socks_proxy"},
            {"key":"mta-out-smtp-username","generator":"static:sunbeam"},
            {"key":"mta-out-smtp-password","generator":"rand_token"}
        ]}),
        json!({"service":"collabora","fields":[
            {"key":"username","generator":"static:admin"},
            {"key":"password","generator":"rand_token"}
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
        "hydra", "kratos", "seaweedfs", "gitea", "hive", "livekit",
        "people", "login-ui", "kratos-admin", "docs", "meet", "drive",
        "projects", "calendars", "messages", "collabora", "tuwunel",
        "grafana", "scaleway-s3",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_configs_have_service_and_fields() {
        for cfg in all_service_configs() {
            assert!(cfg.get("service").is_some(), "missing service in {cfg}");
            assert!(cfg.get("fields").and_then(|f| f.as_array()).is_some(), "missing fields in {cfg}");
        }
    }

    #[test]
    fn service_count() {
        // 18 independent + 1 kratos-admin (dependent)
        assert_eq!(all_service_configs().len(), 18);
        assert_eq!(all_service_names().len(), 19);
    }

    #[test]
    fn kratos_admin_has_from_creds() {
        let cfg = kratos_admin_config();
        let fields = cfg["fields"].as_array().unwrap();
        let s3_field = fields.iter().find(|f| f["key"] == "s3-access-key").unwrap();
        assert!(s3_field["generator"].as_str().unwrap().starts_with("from_creds:"));
    }
}
