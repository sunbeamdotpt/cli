use bytes::Bytes;
use h2::client::SendRequest;
use tokio::net::TcpStream;

use crate::config::VpnConfig;
use crate::keys::NodeKeys;
use crate::noise;

/// Client for the coordination server control protocol.
///
/// Communicates over HTTP/2 on top of a Noise-encrypted TCP stream.
pub struct ControlClient {
    pub(crate) sender: SendRequest<Bytes>,
    /// Keep the h2 connection driver alive.
    _conn_task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient").finish_non_exhaustive()
    }
}

impl ControlClient {
    /// Connect to the coordination server.
    ///
    /// 1. TCP connect to `coordination_url`
    /// 2. Perform TS2021 Noise IK handshake
    /// 3. Wrap the TCP stream in a [`noise::stream::NoiseStream`]
    /// 4. Run `h2::client::handshake` over the encrypted stream
    /// 5. Spawn the h2 connection driver task
    pub async fn connect(config: &VpnConfig, keys: &NodeKeys) -> crate::Result<Self> {
        let addr = parse_coordination_addr(&config.coordination_url)?;
        let use_tls = config.coordination_url.starts_with("https://");
        let host = addr.split(':').next().unwrap_or(&addr).to_string();

        tracing::debug!("connecting to coordination server at {addr} (tls={use_tls})");

        let tls_mode = if config.derp_tls_insecure {
            crate::tls::TlsMode::InsecureSkipVerify
        } else {
            crate::tls::TlsMode::Verify
        };

        // Resolve the server's Noise public key.
        let server_public = match config.server_public_key {
            Some(key) => key,
            None => fetch_server_key(&config.coordination_url, &addr, tls_mode).await?,
        };
        let server_pub_key = x25519_dalek::PublicKey::from(server_public);

        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| crate::Error::Control(format!("tcp connect to {addr}: {e}")))?;

        // Run the handshake either directly on the TCP stream (HTTP coordination)
        // or on top of a TLS-wrapped stream (HTTPS coordination), then hand the
        // resulting NoiseStream to h2.
        if use_tls {
            let mut tls = crate::tls::tls_wrap(tcp, &host, tls_mode).await?;
            let result = noise::handshake::perform_handshake(
                &mut tls,
                &host,
                &keys.node_private,
                &keys.node_public,
                &server_pub_key,
            )
            .await?;
            let noise_stream = noise::stream::NoiseStream::new(
                tls,
                result.tx_cipher,
                result.rx_cipher,
                result.leftover,
            );
            Self::finish_h2_handshake(noise_stream).await
        } else {
            let mut tcp = tcp;
            let result = noise::handshake::perform_handshake(
                &mut tcp,
                &host,
                &keys.node_private,
                &keys.node_public,
                &server_pub_key,
            )
            .await?;
            let noise_stream = noise::stream::NoiseStream::new(
                tcp,
                result.tx_cipher,
                result.rx_cipher,
                result.leftover,
            );
            Self::finish_h2_handshake(noise_stream).await
        }
    }

    /// Common tail for `connect`: consume the early Noise payload, run the
    /// h2 client handshake on top of the encrypted stream, and spawn the
    /// connection driver. Generic over the underlying transport so it
    /// works for both plain TCP and TLS-wrapped connections.
    async fn finish_h2_handshake<S>(
        mut noise_stream: noise::stream::NoiseStream<S>,
    ) -> crate::Result<Self>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let early = noise_stream
            .consume_early_payload()
            .await
            .map_err(|e| crate::Error::Control(format!("early payload: {e}")))?;
        tracing::debug!("early payload consumed ({} bytes)", early.len());

        let (sender, connection) = h2::client::handshake(noise_stream)
            .await
            .map_err(|e| crate::Error::Control(format!("h2 handshake: {e}")))?;

        let conn_task = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("h2 connection error: {e}");
            }
        });

        tracing::debug!("control client connected");

        Ok(Self {
            sender,
            _conn_task: conn_task,
        })
    }

    /// Send a POST request with a JSON body and discard the response body.
    /// Used for endpoints that return an empty 200 (e.g. Headscale's Lite
    /// endpoint update).
    pub(crate) async fn post_json_no_response<Req: serde::Serialize>(
        &mut self,
        path: &str,
        body: &Req,
    ) -> crate::Result<()> {
        let body_bytes = serde_json::to_vec(body)?;
        let request = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(())
            .map_err(|e| crate::Error::Control(e.to_string()))?;

        let (response_future, mut send_stream) = self
            .sender
            .send_request(request, false)
            .map_err(|e| crate::Error::Control(format!("send request: {e}")))?;

        send_stream
            .send_data(Bytes::from(body_bytes), true)
            .map_err(|e| crate::Error::Control(format!("send body: {e}")))?;

        let response = response_future
            .await
            .map_err(|e| crate::Error::Control(format!("response: {e}")))?;

        let status = response.status();
        let mut body = response.into_body();

        // Drain the response body so flow control completes cleanly, but
        // don't try to parse it.
        let mut response_bytes = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|e| crate::Error::Control(format!("read body: {e}")))?;
            response_bytes.extend_from_slice(&chunk);
            body.flow_control()
                .release_capacity(chunk.len())
                .map_err(|e| crate::Error::Control(format!("flow control: {e}")))?;
        }

        if !status.is_success() {
            let text = String::from_utf8_lossy(&response_bytes);
            return Err(crate::Error::Control(format!(
                "{path} returned {status}: {text}"
            )));
        }
        Ok(())
    }

    /// Send a POST request with a JSON body and parse a JSON response.
    pub(crate) async fn post_json<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &mut self,
        path: &str,
        body: &Req,
    ) -> crate::Result<Resp> {
        let body_bytes = serde_json::to_vec(body)?;

        let request = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(())
            .map_err(|e| crate::Error::Control(e.to_string()))?;

        let (response_future, mut send_stream) = self
            .sender
            .send_request(request, false)
            .map_err(|e| crate::Error::Control(format!("send request: {e}")))?;

        // Send the JSON body and signal end-of-stream.
        send_stream
            .send_data(Bytes::from(body_bytes), true)
            .map_err(|e| crate::Error::Control(format!("send body: {e}")))?;

        // Await the response headers.
        let response = response_future
            .await
            .map_err(|e| crate::Error::Control(format!("response: {e}")))?;

        let status = response.status();
        let mut body = response.into_body();

        // Collect the response body.
        let mut response_bytes = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|e| crate::Error::Control(format!("read body: {e}")))?;
            response_bytes.extend_from_slice(&chunk);
            body.flow_control()
                .release_capacity(chunk.len())
                .map_err(|e| crate::Error::Control(format!("flow control: {e}")))?;
        }

        if !status.is_success() {
            let text = String::from_utf8_lossy(&response_bytes);
            return Err(crate::Error::Control(format!(
                "{path} returned {status}: {text}"
            )));
        }

        tracing::debug!("{path} response: {}", String::from_utf8_lossy(&response_bytes));
        let parsed = serde_json::from_slice(&response_bytes)?;
        Ok(parsed)
    }
}

/// Parse a coordination URL into a `host:port` address string.
///
/// Accepts forms like `https://control.example.com`, `http://host:8080`,
/// or plain `host:port`.
fn parse_coordination_addr(url: &str) -> crate::Result<String> {
    // Strip scheme if present.
    let without_scheme = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        url
    };

    // Strip trailing path.
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);

    // Add default port if not present.
    if host_port.contains(':') {
        Ok(host_port.to_string())
    } else if url.starts_with("http://") {
        Ok(format!("{host_port}:80"))
    } else {
        // Default to 443 for https or unspecified.
        Ok(format!("{host_port}:443"))
    }
}

/// Fetch the server's Noise public key from its `/key?v=69` endpoint.
///
/// Headscale/Tailscale returns JSON: `{"publicKey":"mkey:<hex>","legacyPublicKey":"mkey:..."}`.
/// We parse the `publicKey` field and strip the `mkey:` prefix.
async fn fetch_server_key(
    coordination_url: &str,
    addr: &str,
    tls_mode: crate::tls::TlsMode,
) -> crate::Result<[u8; 32]> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let use_tls = coordination_url.starts_with("https://");
    let host = addr.split(':').next().unwrap_or(addr);
    let request = format!(
        "GET /key?v=69 HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );

    let buf = if use_tls {
        let tcp = TcpStream::connect(addr)
            .await
            .map_err(|e| crate::Error::Control(format!("connect to /key: {e}")))?;
        let mut tls = crate::tls::tls_wrap(tcp, host, tls_mode).await?;
        tls.write_all(request.as_bytes())
            .await
            .map_err(|e| crate::Error::Control(format!("write /key request: {e}")))?;
        let mut buf = Vec::with_capacity(4096);
        tls.read_to_end(&mut buf)
            .await
            .map_err(|e| crate::Error::Control(format!("read /key response: {e}")))?;
        buf
    } else {
        let mut tcp = TcpStream::connect(addr)
            .await
            .map_err(|e| crate::Error::Control(format!("connect to /key: {e}")))?;
        tcp.write_all(request.as_bytes())
            .await
            .map_err(|e| crate::Error::Control(format!("write /key request: {e}")))?;
        let mut buf = Vec::with_capacity(4096);
        tcp.read_to_end(&mut buf)
            .await
            .map_err(|e| crate::Error::Control(format!("read /key response: {e}")))?;
        buf
    };

    let response = String::from_utf8_lossy(&buf);

    // Find the body after the blank line.
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .ok_or_else(|| crate::Error::Control("malformed /key response".into()))?
        .trim();

    // Parse JSON response: {"publicKey":"mkey:<hex>", ...}
    let json: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| crate::Error::Control(format!("parse /key JSON: {e} body={body}")))?;

    let public_key = json
        .get("publicKey")
        .or_else(|| json.get("public_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::Error::Control("no publicKey in /key response".into()))?;

    // Strip known prefixes: "mkey:", "nodekey:"
    let hex_str = public_key
        .strip_prefix("mkey:")
        .or_else(|| public_key.strip_prefix("nodekey:"))
        .unwrap_or(public_key);

    parse_hex_key(hex_str)
}

/// Parse a 32-byte key from a hex string.
fn parse_hex_key(hex: &str) -> crate::Result<[u8; 32]> {
    if hex.len() != 64 {
        return Err(crate::Error::Control(format!(
            "expected 64 hex chars for server key, got {}",
            hex.len()
        )));
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| crate::Error::Control(format!("bad hex in server key: {e}")))?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_https_url() {
        let addr = parse_coordination_addr("https://control.example.com").unwrap();
        assert_eq!(addr, "control.example.com:443");
    }

    #[test]
    fn parse_http_url_with_port() {
        let addr = parse_coordination_addr("http://localhost:8080").unwrap();
        assert_eq!(addr, "localhost:8080");
    }

    #[test]
    fn parse_url_with_path() {
        let addr = parse_coordination_addr("https://control.example.com/ts2021").unwrap();
        assert_eq!(addr, "control.example.com:443");
    }

    #[test]
    fn parse_plain_host_port() {
        let addr = parse_coordination_addr("10.0.0.1:443").unwrap();
        assert_eq!(addr, "10.0.0.1:443");
    }

    #[test]
    fn parse_hex_key_valid() {
        let hex = "ab".repeat(32);
        let key = parse_hex_key(&hex).unwrap();
        assert_eq!(key, [0xab; 32]);
    }

    #[test]
    fn parse_hex_key_wrong_length() {
        assert!(parse_hex_key("aabb").is_err());
    }
}
