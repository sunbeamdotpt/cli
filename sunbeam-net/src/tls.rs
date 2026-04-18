//! Shared TLS and TCP helpers used by both the control client (TS2021
//! over HTTPS) and the DERP relay client.
//!
//! Both clients accept either plain TCP (`http://...`) or TLS-wrapped
//! TCP (`https://...`). For production they verify against the system's
//! webpki roots; for tests against self-signed certs they accept any
//! cert via [`TlsMode::InsecureSkipVerify`].

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;

use crate::error::Error;

/// TLS verification mode shared by all clients in this crate.
#[derive(Debug, Clone, Copy, Default)]
pub enum TlsMode {
    /// Standard verification using the system's webpki roots. Use this in
    /// production.
    #[default]
    Verify,
    /// Skip certificate verification. Only use this against a known test
    /// server with a self-signed cert.
    InsecureSkipVerify,
}

/// Wrap a TcpStream in a TLS connection. Honors `TlsMode`.
pub async fn tls_wrap(
    tcp: TcpStream,
    server_name: &str,
    mode: TlsMode,
) -> crate::Result<TlsStream<TcpStream>> {
    let config = match mode {
        TlsMode::Verify => {
            let mut roots = tokio_rustls::rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        }
        TlsMode::InsecureSkipVerify => ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerification))
            .with_no_client_auth(),
    };

    let connector = TlsConnector::from(Arc::new(config));
    let dns_name = ServerName::try_from(server_name.to_string())
        .map_err(|e| Error::Tls(format!("invalid TLS server name {server_name}: {e}")))?;
    connector
        .connect(dns_name, tcp)
        .await
        .map_err(|e| Error::Tls(format!("TLS handshake failed: {e}")))
}

/// Cert verifier that accepts every server certificate. Used when
/// `TlsMode::InsecureSkipVerify` is set.
#[derive(Debug)]
struct NoCertVerification;

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::ServerCertVerified,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> std::result::Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        use tokio_rustls::rustls::SignatureScheme;
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

/// TCP connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Happy Eyeballs (RFC 8305) TCP connect. Resolves all addresses for the
/// given host:port, tries IPv4 and IPv6 concurrently with per-address
/// timeouts. Returns the first successful connection.
pub async fn happy_eyeballs_connect(addr: &str) -> std::io::Result<TcpStream> {
    use std::net::{SocketAddr, ToSocketAddrs};

    let addrs: Vec<SocketAddr> = addr.to_socket_addrs()?.collect();
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            format!("no addresses resolved for {addr}"),
        ));
    }

    // Sort: IPv4 first (more reliable on dual-stack with broken IPv6),
    // then IPv6. Each address gets an individual attempt with the full
    // timeout, but we race ALL of them concurrently so the fastest wins.
    let mut sorted = addrs.clone();
    sorted.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });

    // Race all addresses concurrently. First to connect wins.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::io::Result<TcpStream>>(sorted.len());
    for target in &sorted {
        let target = *target;
        let tx = tx.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await;
            let result = match result {
                Ok(r) => r,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("connect to {target} timed out"),
                )),
            };
            let _ = tx.send(result).await;
        });
    }
    drop(tx); // close sender so rx.recv() returns None when all tasks finish

    let mut last_err = std::io::Error::new(
        std::io::ErrorKind::AddrNotAvailable,
        format!("no addresses for {addr}"),
    );

    while let Some(result) = rx.recv().await {
        match result {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = e,
        }
    }

    Err(last_err)
}
