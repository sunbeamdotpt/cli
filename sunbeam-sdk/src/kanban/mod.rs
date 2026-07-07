//! Kanban project management via gRPC.

use crate::error::{Result, ResultExt, SunbeamError};

pub mod aggregated;
pub mod attachments;
pub mod boards;
pub mod card_templates;
pub mod cards;
pub mod client;
pub mod github_links;
pub mod projects;
pub mod public_boards;
pub mod resolve;
pub mod search;
pub mod subscribe;
pub mod templates;

/// Generate a fresh ULID idempotency key for mutating RPCs.
pub fn new_idempotency_key() -> String {
    ulid::Ulid::new().to_string()
}

/// Resolve the default Kanban server URL from the provided context.
pub fn default_server_url_for(ctx: &crate::config::Context) -> Result<String> {
    let domain = ctx.domain.clone();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "no domain configured; set one with `sunbeam config set --domain ...` or pass --url",
        ));
    }
    Ok(format!("https://kanban.{domain}"))
}

/// Resolve the default Kanban server URL from the active context.
pub fn default_server_url() -> Result<String> {
    default_server_url_for(crate::config::active_context())
}

/// Resolve the final server URL from an explicit override or the active context.
pub fn resolve_server_url(url_override: Option<&str>) -> Result<String> {
    match url_override {
        Some(u) => Ok(u.to_string()),
        None => default_server_url(),
    }
}

/// Resolve and validate a bearer token for authenticated RPCs.
pub async fn require_token() -> Result<String> {
    crate::auth::get_token()
        .await
        .with_ctx(|| "run `sunbeam auth login` first".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_server_url_uses_override() {
        assert_eq!(
            resolve_server_url(Some("http://local")).unwrap(),
            "http://local"
        );
    }

    #[test]
    fn default_server_url_for_builds_from_domain() {
        let ctx = crate::config::Context {
            domain: "sunbeam.test".into(),
            ..Default::default()
        };
        assert_eq!(
            default_server_url_for(&ctx).unwrap(),
            "https://kanban.sunbeam.test"
        );
    }

    #[test]
    fn default_server_url_for_errors_when_domain_empty() {
        let ctx = crate::config::Context::default();
        assert!(default_server_url_for(&ctx).is_err());
    }

    #[test]
    fn new_idempotency_key_is_ulid() {
        let key = new_idempotency_key();
        assert!(!key.is_empty());
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn default_server_url_errors_when_domain_empty() {
        crate::config::set_active_context(crate::config::Context::default());
        let err = default_server_url().unwrap_err();
        assert!(err.to_string().contains("no domain configured"));
    }
}
