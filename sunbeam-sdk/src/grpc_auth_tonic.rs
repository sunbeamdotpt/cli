//! Tonic gRPC plumbing for wfe-server: bearer-token interceptor + channel builder.
//!
//! This module exists solely for `wfectl`, which speaks to `wfe-server-protos`
//! — a tonic-generated service that has not yet been migrated to connectrpc.
//! All gitserv-facing gRPC uses [`crate::grpc_auth`] (connectrpc-based).

use anyhow::{Context, Result, anyhow};
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::{Channel, Endpoint};

/// Tonic interceptor that injects an `Authorization: Bearer <token>` header
/// on every gRPC request. An empty token produces a no-op interceptor.
#[derive(Clone)]
pub struct BearerAuth {
    header: Option<MetadataValue<tonic::metadata::Ascii>>,
}

impl BearerAuth {
    pub fn new(token: &str) -> Result<Self> {
        if token.is_empty() {
            return Ok(Self { header: None });
        }
        let value = format!("Bearer {token}");
        let header = MetadataValue::try_from(value)
            .map_err(|e| anyhow!("invalid auth token (cannot encode as header): {e}"))?;
        Ok(Self {
            header: Some(header),
        })
    }
}

impl Interceptor for BearerAuth {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(header) = &self.header {
            req.metadata_mut().insert("authorization", header.clone());
        }
        Ok(req)
    }
}

/// Build a tonic channel for the given server URL.
///
/// TLS configuration requires enabling tonic's `tls-native-roots` feature in
/// the workspace. Currently wfe-server connections are expected to be plaintext
/// (cluster-internal). Add the feature and `ClientTlsConfig` when needed.
pub async fn connect(server: &str) -> Result<Channel> {
    let endpoint = Endpoint::from_shared(server.to_string())
        .with_context(|| format!("invalid server URL: {server}"))?;

    endpoint
        .connect()
        .await
        .with_context(|| format!("failed to connect to {server}"))
}

/// Build a channel + interceptor pair ready to wrap a generated tonic client.
pub async fn connect_with_bearer(server: &str, token: &str) -> Result<(Channel, BearerAuth)> {
    let channel = connect(server).await?;
    let auth = BearerAuth::new(token)?;
    Ok((channel, auth))
}
