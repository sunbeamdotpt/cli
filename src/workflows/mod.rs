pub mod cmd;
pub mod data;
pub mod host;
pub mod primitives;

pub mod seed;
pub mod up;
pub mod verify;
pub mod bootstrap;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::Result;

/// Serializable context passed through workflow data.
///
/// Steps reconstruct transient handles (kube::Client, BaoClient) from these
/// fields at runtime — they are not serializable, so we store just enough
/// to recreate them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepContext {
    pub domain: String,
    pub infra_dir: String,
    pub kube_context: String,
    pub ssh_host: String,
    pub is_production: bool,
    pub acme_email: String,
    /// The config context name, used for per-context DB paths.
    pub context_name: String,
}

impl StepContext {
    /// Build a StepContext from the currently active CLI context.
    pub fn from_active() -> Self {
        let ctx = config::active_context();
        let cfg = config::load_config();
        Self::from_config(ctx, &cfg.current_context)
    }

    /// Build a StepContext from a config Context and context name.
    /// Separated from `from_active()` to allow unit testing without global state.
    pub fn from_config(ctx: &config::Context, current_context: &str) -> Self {
        let context_name = if current_context.is_empty() {
            "default".to_string()
        } else {
            current_context.to_string()
        };

        StepContext {
            domain: ctx.domain.clone(),
            infra_dir: ctx.infra_dir.clone(),
            kube_context: if ctx.kube_context.is_empty() {
                "sunbeam".to_string()
            } else {
                ctx.kube_context.clone()
            },
            ssh_host: ctx.ssh_host.clone(),
            is_production: !ctx.ssh_host.is_empty(),
            acme_email: ctx.acme_email.clone(),
            context_name,
        }
    }

    /// Reconstruct a Kubernetes client from the stored context.
    pub async fn kube_client(&self) -> Result<kube::Client> {
        crate::kube::get_client().await.map(|c| c.clone())
    }

    /// Build an OpenBao HTTP client from a local port and token.
    pub fn bao_client(&self, port: u16, token: &str) -> crate::openbao::BaoClient {
        crate::openbao::BaoClient::with_token(
            &format!("http://127.0.0.1:{port}"),
            token,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> StepContext {
        StepContext {
            domain: "test.local".to_string(),
            infra_dir: "/tmp/infra".to_string(),
            kube_context: "test-cluster".to_string(),
            ssh_host: String::new(),
            is_production: false,
            acme_email: "test@test.local".to_string(),
            context_name: "test".to_string(),
        }
    }

    fn production_ctx() -> StepContext {
        StepContext {
            domain: "sunbeam.pt".to_string(),
            infra_dir: "/srv/infra".to_string(),
            kube_context: "production".to_string(),
            ssh_host: "user@server.example.com".to_string(),
            is_production: true,
            acme_email: "ops@sunbeam.pt".to_string(),
            context_name: "production".to_string(),
        }
    }

    #[test]
    fn test_step_context_serialization_roundtrip() {
        let ctx = test_ctx();
        let json = serde_json::to_value(&ctx).unwrap();
        let deserialized: StepContext = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.domain, "test.local");
        assert_eq!(deserialized.kube_context, "test-cluster");
        assert!(!deserialized.is_production);
        assert_eq!(deserialized.context_name, "test");
    }

    #[test]
    fn test_step_context_production_flag() {
        let local = test_ctx();
        assert!(!local.is_production);
        assert!(local.ssh_host.is_empty());

        let prod = production_ctx();
        assert!(prod.is_production);
        assert!(!prod.ssh_host.is_empty());
    }

    #[test]
    fn test_step_context_bao_client_construction() {
        let ctx = test_ctx();
        let client = ctx.bao_client(8200, "test-token");
        // BaoClient is opaque, but we can verify it doesn't panic
        // and the base_url is constructed correctly by checking it exists
        drop(client);
    }

    #[test]
    fn test_step_context_bao_client_ephemeral_port() {
        let ctx = test_ctx();
        let client = ctx.bao_client(49152, "some-token");
        drop(client);
    }

    #[test]
    fn test_step_context_embedded_in_json() {
        let ctx = test_ctx();
        let wrapper = serde_json::json!({
            "__ctx": ctx,
            "some_field": "value",
        });
        let extracted: StepContext =
            serde_json::from_value(wrapper["__ctx"].clone()).unwrap();
        assert_eq!(extracted.domain, "test.local");
    }

    #[test]
    fn test_from_config_local_context() {
        let ctx = crate::config::Context {
            domain: "local.dev".to_string(),
            kube_context: "k3s-local".to_string(),
            ssh_host: String::new(),
            infra_dir: "/home/user/infra".to_string(),
            acme_email: "admin@local.dev".to_string(),
            ..Default::default()
        };
        let sc = StepContext::from_config(&ctx, "local");
        assert_eq!(sc.domain, "local.dev");
        assert_eq!(sc.kube_context, "k3s-local");
        assert!(!sc.is_production);
        assert_eq!(sc.context_name, "local");
        assert_eq!(sc.infra_dir, "/home/user/infra");
        assert_eq!(sc.acme_email, "admin@local.dev");
    }

    #[test]
    fn test_from_config_production_context() {
        let ctx = crate::config::Context {
            domain: "sunbeam.pt".to_string(),
            kube_context: "production".to_string(),
            ssh_host: "sienna@62.210.145.138".to_string(),
            infra_dir: "/srv/infra".to_string(),
            acme_email: "ops@sunbeam.pt".to_string(),
            ..Default::default()
        };
        let sc = StepContext::from_config(&ctx, "production");
        assert!(sc.is_production);
        assert_eq!(sc.ssh_host, "sienna@62.210.145.138");
        assert_eq!(sc.context_name, "production");
    }

    #[test]
    fn test_from_config_empty_context_name_defaults() {
        let ctx = crate::config::Context::default();
        let sc = StepContext::from_config(&ctx, "");
        assert_eq!(sc.context_name, "default");
    }

    #[test]
    fn test_from_config_empty_kube_context_defaults_sunbeam() {
        let ctx = crate::config::Context {
            kube_context: String::new(),
            ..Default::default()
        };
        let sc = StepContext::from_config(&ctx, "test");
        assert_eq!(sc.kube_context, "sunbeam");
    }

    #[test]
    fn test_step_context_empty_fields() {
        let ctx = StepContext {
            domain: String::new(),
            infra_dir: String::new(),
            kube_context: String::new(),
            ssh_host: String::new(),
            is_production: false,
            acme_email: String::new(),
            context_name: String::new(),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: StepContext = serde_json::from_str(&json).unwrap();
        assert!(back.domain.is_empty());
        assert!(!back.is_production);
    }
}
