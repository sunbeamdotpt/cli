//! Workflow data types (seed, up, verify, bootstrap).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::StepContext;

/// Workflow data for the `seed` workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeedData {
    /// Shared CLI context (domain, kube context, etc.)
    #[serde(default, rename = "__ctx")]
    pub ctx: Option<StepContext>,

    // -- Phase 1: OpenBao init --
    /// Ob pod.
    pub ob_pod: Option<String>,
    /// Ob port.
    pub ob_port: Option<u16>,
    /// Root token.
    pub root_token: Option<String>,
    /// Initialized.
    pub initialized: Option<bool>,
    /// Sealed.
    pub sealed: Option<bool>,
    /// Skip seed.
    #[serde(default)]
    pub skip_seed: bool,
    /// Skip Ory (Hydra/Kratos/Keto) namespace — used on Lima VMs where
    /// the full identity stack overwhelms single-node k3s.
    #[serde(default)]
    pub skip_ory: bool,

    // -- Phase 2: KV seeding --
    /// Accumulated credential values keyed by "path/field".
    #[serde(default)]
    pub creds: HashMap<String, String>,
    /// KV paths that were modified and need writing.
    #[serde(default)]
    pub dirty_paths: Vec<String>,

    // -- Phase 4: PostgreSQL --
    /// Pg pod.
    pub pg_pod: Option<String>,

    // -- Phase 6: Kratos admin --
    /// Recovery link.
    pub recovery_link: Option<String>,
    /// Recovery code.
    pub recovery_code: Option<String>,
    /// Dkim public key.
    pub dkim_public_key: Option<String>,
    /// Admin identity id.
    pub admin_identity_id: Option<String>,
}

/// Workflow data for the `up` workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpData {
    #[serde(default, rename = "__ctx")]
    /// Ctx.
    pub ctx: Option<StepContext>,
    #[serde(default)]
    /// Domain.
    pub domain: String,

    // -- Vault phase (reused from seed) --
    /// Ob pod.
    pub ob_pod: Option<String>,
    /// Ob port.
    pub ob_port: Option<u16>,
    /// Root token.
    pub root_token: Option<String>,
    #[serde(default)]
    /// Skip seed.
    pub skip_seed: bool,
    #[serde(default)]
    /// Creds.
    pub creds: HashMap<String, String>,
    #[serde(default)]
    /// Dirty paths.
    pub dirty_paths: Vec<String>,

    // -- Postgres phase --
    /// Pg pod.
    pub pg_pod: Option<String>,

    // -- Infrastructure phase --
    /// Skip Cilium check.
    #[serde(default)]
    pub skip_cilium: bool,
    /// Skip Ory (Hydra/Kratos/Keto) namespace — used on resource-constrained
    /// environments where the full identity stack overwhelms single-node k3s.
    #[serde(default)]
    pub skip_ory: bool,
    /// Namespaces to skip (to reduce resource pressure on constrained clusters).
    #[serde(default)]
    pub skip_namespaces: Vec<String>,
    /// Run in serial mode: longer delays and more conservative resource usage
    /// for tiny single-node clusters.
    #[serde(default)]
    pub serial_mode: bool,
}

/// Workflow data for the `verify` workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerifyData {
    #[serde(default, rename = "__ctx")]
    /// Ctx.
    pub ctx: Option<StepContext>,
    /// Ob pod.
    pub ob_pod: Option<String>,
    /// Ob port.
    pub ob_port: Option<u16>,
    /// Root token.
    pub root_token: Option<String>,
    /// Test value.
    pub test_value: Option<String>,
    /// Synced.
    pub synced: bool,
}

/// Workflow data for the `down` workflow.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownData {
    #[serde(default, rename = "__ctx")]
    /// Ctx.
    pub ctx: Option<StepContext>,
    /// Also delete infrastructure namespaces.
    pub infra: bool,
    /// Preserve data namespace.
    pub keep_data: bool,
    /// Namespaces discovered for deletion.
    #[serde(default)]
    pub namespaces_to_delete: Vec<String>,
    /// Namespaces still stuck after wait.
    #[serde(default)]
    pub remaining_namespaces: Vec<String>,
}

/// Workflow data for the `bootstrap` workflow.
///
/// Deprecated: bootstrap workflow has been merged into `up`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BootstrapData {
    #[serde(default, rename = "__ctx")]
    /// Ctx.
    pub ctx: Option<StepContext>,
    /// Domain.
    pub domain: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_ctx() -> StepContext {
        StepContext {
            domain: "test.local".to_string(),
            infra_dir: "/tmp".to_string(),
            kube_context: "test".to_string(),
            acme_email: String::new(),
            context_name: "default".to_string(),
        }
    }

    // -- SeedData --

    #[test]
    fn test_seed_data_default() {
        let d = SeedData::default();
        assert!(d.ctx.is_none());
        assert!(d.ob_pod.is_none());
        assert!(!d.skip_seed);
        assert!(d.creds.is_empty());
        assert!(d.dirty_paths.is_empty());
    }

    #[test]
    fn test_seed_data_serialization_roundtrip() {
        let mut creds = HashMap::new();
        creds.insert("hydra/system-secret".to_string(), "abc123".to_string());
        creds.insert("kratos/cookie-secret".to_string(), "xyz789".to_string());

        let d = SeedData {
            ctx: Some(make_ctx()),
            ob_pod: Some("openbao-0".to_string()),
            ob_port: Some(8200),
            root_token: Some("hvs.test".to_string()),
            initialized: Some(true),
            sealed: Some(false),
            skip_seed: false,
            skip_ory: false,
            creds,
            dirty_paths: vec!["hydra".to_string()],
            pg_pod: Some("postgres-1".to_string()),
            recovery_link: None,
            recovery_code: None,
            dkim_public_key: Some("MIIBIjAN...".to_string()),
            admin_identity_id: None,
        };

        let json = serde_json::to_value(&d).unwrap();
        let back: SeedData = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(back.ob_pod.as_deref(), Some("openbao-0"));
        assert_eq!(back.ob_port, Some(8200));
        assert_eq!(back.root_token.as_deref(), Some("hvs.test"));
        assert_eq!(back.initialized, Some(true));
        assert_eq!(back.sealed, Some(false));
        assert_eq!(back.creds.len(), 2);
        assert_eq!(back.creds["hydra/system-secret"], "abc123");
        assert_eq!(back.dirty_paths, vec!["hydra"]);
        assert_eq!(back.pg_pod.as_deref(), Some("postgres-1"));
        assert_eq!(back.dkim_public_key.as_deref(), Some("MIIBIjAN..."));

        // __ctx rename should work
        assert!(json.get("__ctx").is_some());
        assert!(json.get("ctx").is_none());
    }

    #[test]
    fn test_seed_data_ctx_rename() {
        let d = SeedData {
            ctx: Some(make_ctx()),
            ..Default::default()
        };
        let json = serde_json::to_value(&d).unwrap();
        // The field should be serialized as "__ctx", not "ctx"
        assert!(json.get("__ctx").is_some());
        assert!(json.get("ctx").is_none());
        // And deserializes back
        let back: SeedData = serde_json::from_value(json).unwrap();
        assert!(back.ctx.is_some());
        assert_eq!(back.ctx.unwrap().domain, "test.local");
    }

    #[test]
    fn test_seed_data_from_json_without_ctx() {
        // Workflow data might not have __ctx initially
        let json = serde_json::json!({
            "ob_pod": "openbao-0",
            "skip_seed": true,
        });
        let d: SeedData = serde_json::from_value(json).unwrap();
        assert!(d.ctx.is_none());
        assert_eq!(d.ob_pod.as_deref(), Some("openbao-0"));
        assert!(d.skip_seed);
    }

    // -- UpData --

    #[test]
    fn test_up_data_default() {
        let d = UpData::default();
        assert!(d.ctx.is_none());
        assert!(d.domain.is_empty());
        assert!(!d.skip_seed);
        assert!(!d.skip_cilium);
        assert!(d.ob_pod.is_none());
        assert!(d.creds.is_empty());
        assert!(d.pg_pod.is_none());
    }

    #[test]
    fn test_up_data_roundtrip() {
        let d = UpData {
            ctx: Some(make_ctx()),
            domain: "sunbeam.pt".to_string(),
            ob_pod: Some("openbao-0".to_string()),
            ob_port: Some(8200),
            root_token: Some("hvs.test".to_string()),
            skip_seed: false,
            creds: HashMap::new(),
            dirty_paths: vec![],
            pg_pod: Some("postgres-1".to_string()),
            skip_cilium: false,
            skip_ory: false,
            skip_namespaces: vec![],
            serial_mode: false,
        };
        let json = serde_json::to_value(&d).unwrap();
        let back: UpData = serde_json::from_value(json).unwrap();
        assert_eq!(back.domain, "sunbeam.pt");
        assert!(back.ctx.is_some());
        assert_eq!(back.ob_pod.as_deref(), Some("openbao-0"));
        assert_eq!(back.pg_pod.as_deref(), Some("postgres-1"));
    }

    // -- VerifyData --

    #[test]
    fn test_verify_data_default() {
        let d = VerifyData::default();
        assert!(!d.synced);
        assert!(d.test_value.is_none());
    }

    #[test]
    fn test_verify_data_roundtrip() {
        let d = VerifyData {
            ctx: Some(make_ctx()),
            ob_pod: Some("openbao-0".to_string()),
            ob_port: Some(8200),
            root_token: Some("root".to_string()),
            test_value: Some("sentinel-abc".to_string()),
            synced: true,
        };
        let json = serde_json::to_value(&d).unwrap();
        let back: VerifyData = serde_json::from_value(json).unwrap();
        assert!(back.synced);
        assert_eq!(back.test_value.as_deref(), Some("sentinel-abc"));
    }

    // -- BootstrapData --

    #[test]
    fn test_bootstrap_data_default() {
        let d = BootstrapData::default();
        assert!(d.domain.is_none());
    }

    #[test]
    fn test_bootstrap_data_roundtrip() {
        let d = BootstrapData {
            ctx: Some(make_ctx()),
            domain: Some("test.local".to_string()),
        };
        let json = serde_json::to_value(&d).unwrap();
        let back: BootstrapData = serde_json::from_value(json).unwrap();
        assert_eq!(back.domain.as_deref(), Some("test.local"));
    }

    // -- Cross-data-type: ensure WFE can use serde_json::Value as data --

    #[test]
    fn test_seed_data_as_json_value() {
        let d = SeedData {
            ctx: Some(make_ctx()),
            ..Default::default()
        };
        // WFE stores data as serde_json::Value — verify this works
        let val = serde_json::to_value(&d).unwrap();
        assert!(val.is_object());
        // And can be read back
        let back: SeedData = serde_json::from_value(val).unwrap();
        assert!(back.ctx.is_some());
    }
}
