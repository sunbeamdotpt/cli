//! Kanban authentication commands.

use crate::error::Result;
use crate::kanban::client::{self, AuthServiceClient};
use crate::logger::Logger;
use crate::output::{OutputFormat, render};
use async_trait::async_trait;
use clap::Subcommand;
use serde::Serialize;

/// Authentication actions.
#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Show the current SSO identity.
    WhoAmI,
    /// Signal logout and write the Valkey logout watermark.
    Logout,
}

/// Serializable subset of WhoAmIResponse for output.
#[derive(Serialize)]
struct WhoAmIOut {
    subject: String,
    display_name: String,
    email: String,
    roles: Vec<String>,
    expires_at_ms: i64,
}

/// Trait abstracting the Kanban auth service for testability.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AuthService {
    /// Return the current SSO identity.
    async fn who_am_i(&mut self, req: ()) -> Result<client::WhoAmIResponse>;
    /// Signal logout and return the watermark.
    async fn signal_logout(&mut self, req: ()) -> Result<client::SignalLogoutResponse>;
}

/// Wrapper around the generated Tonic auth client.
#[derive(Debug)]
pub struct AuthServiceClientWrapper {
    inner: AuthServiceClient<client::AuthChannel>,
}

impl AuthServiceClientWrapper {
    /// Build a wrapper from an authenticated channel.
    pub fn new(inner: AuthServiceClient<client::AuthChannel>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AuthService for AuthServiceClientWrapper {
    async fn who_am_i(&mut self, _req: ()) -> Result<client::WhoAmIResponse> {
        let resp = self.inner.who_am_i(tonic::Request::new(())).await?;
        Ok(resp.into_inner())
    }

    async fn signal_logout(&mut self, _req: ()) -> Result<client::SignalLogoutResponse> {
        let resp = self.inner.signal_logout(tonic::Request::new(())).await?;
        Ok(resp.into_inner())
    }
}

/// Build an auth-service client wrapper for the given server and token.
pub async fn build_client(
    logger: &Logger,
    server: &str,
    token: &str,
) -> Result<AuthServiceClientWrapper> {
    let channel = client::build(logger, server, token).await?;
    Ok(AuthServiceClientWrapper::new(AuthServiceClient::new(
        channel,
    )))
}

/// Run an auth command.
pub async fn run(
    cmd: AuthAction,
    format: OutputFormat,
    client: &mut dyn AuthService,
) -> Result<()> {
    match cmd {
        AuthAction::WhoAmI => {
            let r = client.who_am_i(()).await?;
            render(
                &WhoAmIOut {
                    subject: r.subject,
                    display_name: r.display_name,
                    email: r.email,
                    roles: r.roles,
                    expires_at_ms: r.expires_at_ms,
                },
                format,
            )
        }
        AuthAction::Logout => {
            let r = client.signal_logout(()).await?;
            render(
                &serde_json::json!({
                    "subject": r.subject,
                    "watermark_ms": r.watermark_ms,
                }),
                format,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputFormat;

    #[tokio::test]
    async fn whoami_renders_identity() {
        let mut mock = MockAuthService::new();
        mock.expect_who_am_i()
            .withf(|req| req == &())
            .times(1)
            .returning(|_| {
                Ok(client::WhoAmIResponse {
                    subject: "sub_123".into(),
                    display_name: "Ada Lovelace".into(),
                    email: "ada@example.com".into(),
                    roles: vec!["admin".into()],
                    expires_at_ms: 1_700_000_000_000,
                })
            });

        run(AuthAction::WhoAmI, OutputFormat::Json, &mut mock)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn logout_renders_watermark() {
        let mut mock = MockAuthService::new();
        mock.expect_signal_logout().times(1).returning(|_| {
            Ok(client::SignalLogoutResponse {
                subject: "sub_123".into(),
                watermark_ms: 1_700_000_000_000,
            })
        });

        run(AuthAction::Logout, OutputFormat::Json, &mut mock)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn whoami_table_renders() {
        let mut mock = MockAuthService::new();
        mock.expect_who_am_i().times(1).returning(|_| {
            Ok(client::WhoAmIResponse {
                subject: "sub_123".into(),
                display_name: "Ada".into(),
                email: "ada@example.com".into(),
                roles: vec!["admin".into()],
                expires_at_ms: 1,
            })
        });

        run(AuthAction::WhoAmI, OutputFormat::Table, &mut mock)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn build_client_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build_client(&logger, ":::bad", "token").await.unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }

    #[tokio::test]
    async fn wrapper_new_constructs() {
        let channel = tonic::transport::Endpoint::from_static("http://[::1]:1").connect_lazy();
        let auth = crate::kanban::client::BearerAuth::new("").unwrap();
        let auth_channel = tonic::service::interceptor::InterceptedService::new(channel, auth);
        let _wrapper = AuthServiceClientWrapper::new(AuthServiceClient::new(auth_channel));
    }
}
