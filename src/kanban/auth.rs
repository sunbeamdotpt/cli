//! Kanban session helpers — whoami and logout.

use clap::Subcommand;
use sdk::error::{Result, SunbeamError};
use serde::Serialize;

use crate::output::{OutputFormat, render};

/// Kanban auth actions.
#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Show the current SSO identity and token status.
    Whoami,
    /// Remove cached tokens (same as `sunbeam auth logout`).
    Logout,
}

/// Serializable whoami output.
#[derive(Serialize)]
struct WhoamiOut {
    domain: String,
    subject: String,
    email: String,
    expires_at: String,
    expired: bool,
}

/// Build the whoami view from cached tokens.
///
/// The subject and email are read from the id_token payload when present,
/// falling back to the access token payload for the subject.
fn whoami_out(domain: &str, tokens: &sdk::config::AuthTokens) -> WhoamiOut {
    let id_claims = tokens
        .id_token
        .as_deref()
        .and_then(|t| crate::auth::decode_jwt_payload(t).ok());
    let access_claims = crate::auth::decode_jwt_payload(&tokens.access_token).ok();

    let claim_str = |key: &str| -> Option<String> {
        id_claims
            .as_ref()
            .and_then(|c| c.get(key))
            .or_else(|| access_claims.as_ref().and_then(|c| c.get(key)))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };

    let now = chrono::Utc::now();
    WhoamiOut {
        domain: domain.to_string(),
        subject: claim_str("sub").unwrap_or_default(),
        email: claim_str("email").unwrap_or_default(),
        expires_at: tokens.expires_at.to_rfc3339(),
        expired: tokens.expires_at <= now,
    }
}

/// Run a kanban auth command.
pub(crate) async fn run(action: AuthAction, format: OutputFormat) -> Result<()> {
    match action {
        AuthAction::Whoami => {
            let domain = sdk::config::domain();
            if domain.is_empty() {
                return Err(SunbeamError::config(
                    "no domain configured; set one with `sunbeam config set --domain ...`",
                ));
            }
            let tokens = sdk::config::get_auth_tokens(domain).ok_or_else(|| {
                SunbeamError::identity("not logged in; run `sunbeam auth login` first")
            })?;
            render(&whoami_out(domain, &tokens), format)
        }
        AuthAction::Logout => crate::auth::cmd_auth_logout().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use chrono::{Duration, Utc};

    fn fake_jwt(payload: &str) -> String {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("eyJhbGciOiJSUzI1NiJ9.{encoded}.sig")
    }

    fn tokens(id_token: Option<String>, expires_in: Duration) -> sdk::config::AuthTokens {
        sdk::config::AuthTokens {
            access_token: fake_jwt(r#"{"sub":"user:access"}"#),
            refresh_token: "refresh".into(),
            expires_at: Utc::now() + expires_in,
            id_token,
        }
    }

    #[test]
    fn whoami_reads_id_token_claims() {
        let tokens = tokens(
            Some(fake_jwt(
                r#"{"sub":"user:alice","email":"alice@sunbeam.pt"}"#,
            )),
            Duration::hours(1),
        );
        let out = whoami_out("sunbeam.test", &tokens);
        assert_eq!(out.domain, "sunbeam.test");
        assert_eq!(out.subject, "user:alice");
        assert_eq!(out.email, "alice@sunbeam.pt");
        assert!(!out.expired);
        assert!(!out.expires_at.is_empty());
    }

    #[test]
    fn whoami_falls_back_to_access_token_subject() {
        let tokens = tokens(None, Duration::hours(1));
        let out = whoami_out("sunbeam.test", &tokens);
        assert_eq!(out.subject, "user:access");
        assert!(out.email.is_empty());
    }

    #[test]
    fn whoami_flags_expired_tokens() {
        let tokens = tokens(None, Duration::hours(-1));
        let out = whoami_out("sunbeam.test", &tokens);
        assert!(out.expired);
    }

    #[test]
    fn whoami_tolerates_non_jwt_tokens() {
        let tokens = sdk::config::AuthTokens {
            access_token: "opaque-token".into(),
            refresh_token: String::new(),
            expires_at: Utc::now() + Duration::hours(1),
            id_token: None,
        };
        let out = whoami_out("sunbeam.test", &tokens);
        assert!(out.subject.is_empty());
        assert!(out.email.is_empty());
    }
}
