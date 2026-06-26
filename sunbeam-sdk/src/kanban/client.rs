//! Kanban gRPC client — Tonic channel, bearer auth, and object-id metadata helpers.

#![allow(missing_docs)]
#![allow(deprecated)]

use crate::error::{Result, ResultExt, SunbeamError};
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

// Bring the generated Tonic/Prost client stubs into this module.
include!(concat!(env!("OUT_DIR"), "/sunbeam.kanban.v1.rs"));

/// Re-export the generated clients so subcommand modules can use them directly.
pub use aggregated_board_service_client::AggregatedBoardServiceClient;
pub use attachment_service_client::AttachmentServiceClient;
pub use auth_service_client::AuthServiceClient;
pub use board_service_client::BoardServiceClient;
pub use card_service_client::CardServiceClient;
pub use github_link_service_client::GithubLinkServiceClient;
pub use project_service_client::ProjectServiceClient;
pub use public_board_service_client::PublicBoardServiceClient;
pub use search_service_client::SearchServiceClient;
pub use templates_service_client::TemplatesServiceClient;

/// Tonic interceptor that injects an `Authorization: Bearer <token>` header.
#[derive(Clone)]
pub struct BearerAuth {
    header: Option<MetadataValue<tonic::metadata::Ascii>>,
}

impl BearerAuth {
    /// Construct a bearer-auth interceptor. An empty token injects nothing.
    pub fn new(token: &str) -> Result<Self> {
        if token.is_empty() {
            return Ok(Self { header: None });
        }
        let value = format!("Bearer {token}");
        let header = MetadataValue::try_from(value)
            .with_ctx(|| "invalid auth token (cannot encode as header)".to_string())?;
        Ok(Self {
            header: Some(header),
        })
    }
}

impl Interceptor for BearerAuth {
    fn call(
        &mut self,
        mut req: tonic::Request<()>,
    ) -> std::result::Result<tonic::Request<()>, tonic::Status> {
        if let Some(header) = &self.header {
            req.metadata_mut().insert("authorization", header.clone());
        }
        Ok(req)
    }
}

/// Type alias for an authenticated Tonic channel.
pub type AuthChannel = InterceptedService<Channel, BearerAuth>;

/// Build a Tonic channel for the given server URL, configuring TLS for https.
pub async fn connect(logger: &crate::logger::Logger, server: &str) -> Result<Channel> {
    crate::debug!(
        logger,
        "kanban client connecting to",
        server = server.to_string()
    );
    let mut endpoint = Endpoint::from_shared(server.to_string())
        .map_err(|e| SunbeamError::Other(format!("invalid kanban server URL {server}: {e}")))?;

    if server.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .map_err(|e| SunbeamError::Network {
                context: format!("failed to configure kanban TLS: {e}"),
                source: None,
            })?;
    }

    endpoint
        .connect()
        .await
        .with_ctx(|| format!("failed to connect to kanban server {server}"))
}

/// Build an authenticated channel (used by all non-public services).
pub async fn build(
    logger: &crate::logger::Logger,
    server: &str,
    token: &str,
) -> Result<AuthChannel> {
    let channel = connect(logger, server).await?;
    let auth = BearerAuth::new(token)?;
    Ok(InterceptedService::new(channel, auth))
}

/// Set the `x-sunbeam-object-id` header on a request.
///
/// Kanban's `keto_dispatch` middleware reads this header for mutating RPCs.
pub fn with_object_id<T>(req: &mut tonic::Request<T>, object_id: &str) -> Result<()> {
    let value = MetadataValue::try_from(object_id.to_string())
        .with_ctx(|| "invalid object id (cannot encode as header)".to_string())?;
    req.metadata_mut().insert("x-sunbeam-object-id", value);
    Ok(())
}

/// Convenience: build a request and attach an object-id header.
pub fn request_with_object_id<T>(msg: T, object_id: &str) -> Result<tonic::Request<T>> {
    let mut req = tonic::Request::new(msg);
    with_object_id(&mut req, object_id)?;
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_auth_with_empty_token_injects_nothing() {
        let mut auth = BearerAuth::new("").unwrap();
        let req = tonic::Request::new(());
        let out = auth.call(req).unwrap();
        assert!(out.metadata().get("authorization").is_none());
    }

    #[test]
    fn bearer_auth_injects_header() {
        let mut auth = BearerAuth::new("ory_at_xyz").unwrap();
        let req = tonic::Request::new(());
        let out = auth.call(req).unwrap();
        let header = out.metadata().get("authorization").unwrap();
        assert_eq!(header.to_str().unwrap(), "Bearer ory_at_xyz");
    }

    #[test]
    fn bearer_auth_rejects_invalid_chars() {
        assert!(BearerAuth::new("bad\ntoken").is_err());
    }

    #[test]
    fn object_id_header_attached() {
        let req = request_with_object_id((), "board_abc123").unwrap();
        assert_eq!(
            req.metadata()
                .get("x-sunbeam-object-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "board_abc123"
        );
    }

    #[test]
    fn object_id_header_rejects_invalid_chars() {
        assert!(with_object_id(&mut tonic::Request::new(()), "bad\nid").is_err());
        assert!(request_with_object_id((), "bad\nid").is_err());
    }

    #[tokio::test]
    async fn connect_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = connect(&logger, "not a valid url ::://").await.unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }

    #[tokio::test]
    async fn build_with_invalid_url_returns_error() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build(&logger, "http://[::1]:1", "token")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("failed to connect") || err.to_string().contains("connect"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn https_connect_configures_tls_and_fails() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            connect(&logger, "https://127.0.0.1:1"),
        )
        .await;
        assert!(result.is_ok(), "connect timed out");
        assert!(result.unwrap().is_err());
    }
}
