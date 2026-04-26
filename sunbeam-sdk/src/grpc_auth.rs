//! Shared Tonic gRPC plumbing: bearer-token interceptor + channel builder.
//!
//! Used by every sunbeam subcommand that speaks gRPC to a sunbeam-owned
//! service (wfectl, vcs, inbox, …). Services layer their own generated
//! client on top via `InterceptedService<Channel, BearerAuth>`.

use anyhow::{Context, Result, anyhow};
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

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

/// Build a tonic channel for the given server URL, enabling TLS with native
/// roots when the scheme is `https`.
pub async fn connect(server: &str) -> Result<Channel> {
    let mut endpoint = Endpoint::from_shared(server.to_string())
        .with_context(|| format!("invalid server URL: {server}"))?;

    if server.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .context("failed to configure TLS")?;
    }

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
        let result = BearerAuth::new("bad\ntoken");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn connect_invalid_url_returns_error() {
        let result = connect("not a valid url").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn connect_to_unreachable_address_fails() {
        let result = connect("http://127.0.0.1:1").await;
        assert!(result.is_err());
    }
}
