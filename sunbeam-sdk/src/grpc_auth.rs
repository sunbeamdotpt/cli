//! Shared connectrpc client plumbing: bearer-token config + connection builder.
//!
//! Used by every sunbeam subcommand that speaks gRPC to a sunbeam-owned
//! service (wfectl, vcs, inbox, …). Services build their generated client
//! on top via `XServiceClient::new(conn, config)`.

use std::sync::Arc;

use anyhow::{Context, Result};
use connectrpc::Protocol;
use connectrpc::client::{ClientConfig, Http2Connection, SharedHttp2Connection};

/// Build a plaintext or TLS HTTP/2 connection for the given server URL.
///
/// TLS is used automatically when the scheme is `https`. The returned
/// `SharedHttp2Connection` is cheap to clone and can be shared across
/// multiple client instances.
pub async fn connect(server: &str) -> Result<SharedHttp2Connection> {
    let uri: http::Uri = server
        .parse()
        .with_context(|| format!("invalid server URL: {server}"))?;

    let conn = if server.starts_with("https://") {
        let tls_config = Arc::new(
            connectrpc::rustls::ClientConfig::builder()
                .with_root_certificates(webpki_roots_store())
                .with_no_client_auth(),
        );
        Http2Connection::connect_tls(uri, tls_config)
            .await
            .map_err(|e| anyhow::anyhow!("TLS connect failed: {e}"))?
    } else {
        Http2Connection::connect_plaintext(uri)
            .await
            .map_err(|e| anyhow::anyhow!("connect failed: {e}"))?
    };

    Ok(conn.shared(256))
}

/// Build a webpki root certificate store from the bundled `webpki-roots` crate.
fn webpki_roots_store() -> connectrpc::rustls::RootCertStore {
    let mut store = connectrpc::rustls::RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}

/// Build a connection + pre-configured [`ClientConfig`] with the bearer token
/// injected as the default `Authorization` header.
pub async fn connect_with_bearer(
    server: &str,
    token: &str,
) -> Result<(SharedHttp2Connection, ClientConfig)> {
    let uri: http::Uri = server
        .parse()
        .with_context(|| format!("invalid server URL: {server}"))?;

    let conn = connect(server).await?;
    let mut config = ClientConfig::new(uri).protocol(Protocol::Grpc);
    if !token.is_empty() {
        config = config.default_header(
            http::header::AUTHORIZATION,
            http::HeaderValue::try_from(format!("Bearer {token}"))
                .with_context(|| "invalid auth token (cannot encode as header)")?,
        );
    }
    Ok((conn, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_invalid_url_returns_error() {
        let result = connect("not a valid url").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn connect_to_unreachable_address_fails() {
        // An unreachable address on a valid-scheme URL should error at connect time.
        let result = connect("http://127.0.0.1:1").await;
        assert!(result.is_err());
    }
}
