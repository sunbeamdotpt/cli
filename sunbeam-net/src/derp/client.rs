use bytes::{BufMut, BytesMut};
use futures::{SinkExt, StreamExt};
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

use super::framing::*;
use crate::error::Error;

/// A trait alias for the underlying transport — either a plain TcpStream
/// (`derp://host` / `http://host`) or a TLS-wrapped one (`https://host`).
trait DerpTransport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> DerpTransport for T {}

/// Client for a single DERP relay server.
pub struct DerpClient {
    inner: Framed<Box<dyn DerpTransport>, DerpFrameCodec>,
    server_public: [u8; 32],
}

/// Re-export of the shared TLS mode under the DERP-specific name so
/// existing call sites keep working.
pub use crate::tls::TlsMode as DerpTlsMode;

impl std::fmt::Debug for DerpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DerpClient")
            .field("server_public", &hex_encode(&self.server_public))
            .finish()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// TLS helpers live in `crate::tls` and are shared with the control client.

impl DerpClient {
    /// Connect to a DERP server, perform HTTP upgrade and NaCl handshake.
    ///
    /// `url` accepts `http://host:port`, `https://host:port`, or bare
    /// `host:port` (treated as plain HTTP). HTTPS uses standard webpki
    /// certificate verification.
    pub async fn connect(
        url: &str,
        node_keys: &crate::keys::NodeKeys,
    ) -> crate::Result<Self> {
        Self::connect_with_tls(url, node_keys, DerpTlsMode::Verify).await
    }

    /// Like [`connect`], but lets the caller pick a TLS verification
    /// mode. Use this with `DerpTlsMode::InsecureSkipVerify` against test
    /// servers with self-signed certs.
    pub async fn connect_with_tls(
        url: &str,
        node_keys: &crate::keys::NodeKeys,
        tls_mode: DerpTlsMode,
    ) -> crate::Result<Self> {
        use crypto_box::aead::{Aead, AeadCore, OsRng};
        use crypto_box::{PublicKey, SalsaBox, SecretKey};

        // Parse the URL into (scheme, addr).
        let (use_tls, addr) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            (false, url)
        };

        // TCP connect
        let tcp = TcpStream::connect(addr).await.map_err(|e| {
            Error::Derp(format!("failed to connect to DERP server {addr}: {e}"))
        })?;

        // Optionally wrap in TLS.
        let host = addr.split(':').next().unwrap_or(addr);
        let mut stream: Box<dyn DerpTransport> = if use_tls {
            Box::new(crate::tls::tls_wrap(tcp, host, tls_mode).await?)
        } else {
            Box::new(tcp)
        };

        // HTTP upgrade request
        let upgrade_req = format!(
            "GET /derp HTTP/1.1\r\n\
             Host: {host}\r\n\
             Connection: Upgrade\r\n\
             Upgrade: DERP\r\n\
             \r\n"
        );

        stream.write_all(upgrade_req.as_bytes()).await.map_err(|e| {
            Error::Derp(format!("failed to send upgrade request: {e}"))
        })?;

        // Read until we see the end-of-headers `\r\n\r\n`. We must NOT use a
        // BufReader: the DERP server sends the first frame inline immediately
        // after the HTTP response, and a BufReader's internal buffer would
        // swallow those bytes when we discard it.
        let mut header_buf = Vec::with_capacity(512);
        loop {
            let mut byte = [0u8; 1];
            let n = stream.read(&mut byte).await.map_err(|e| {
                Error::Derp(format!("failed to read upgrade response: {e}"))
            })?;
            if n == 0 {
                return Err(Error::Derp("connection closed during HTTP upgrade".into()));
            }
            header_buf.push(byte[0]);
            if header_buf.ends_with(b"\r\n\r\n") {
                break;
            }
            if header_buf.len() > 8192 {
                return Err(Error::Derp("HTTP upgrade headers too large".into()));
            }
        }

        let header_str = std::str::from_utf8(&header_buf)
            .map_err(|e| Error::Derp(format!("upgrade headers not utf-8: {e}")))?;
        let status_line = header_str.lines().next().unwrap_or("");
        if !status_line.contains("101") {
            return Err(Error::Derp(format!(
                "DERP server did not send 101 Switching Protocols: {status_line}"
            )));
        }

        // Wrap in framed codec
        let mut framed = Framed::new(stream, DerpFrameCodec);

        // Read ServerKey frame (FRAME_SERVER_KEY, 32 bytes)
        let server_key_frame = framed
            .next()
            .await
            .ok_or_else(|| Error::Derp("connection closed before ServerKey".into()))?
            .map_err(|e| Error::Derp(format!("failed to read ServerKey frame: {e}")))?;

        if server_key_frame.frame_type != FRAME_SERVER_KEY {
            return Err(Error::Derp(format!(
                "expected ServerKey frame (0x01), got 0x{:02x}",
                server_key_frame.frame_type
            )));
        }
        // Tailscale's ServerKey frame is 8 bytes of magic ("DERP🔑") followed
        // by the 32-byte server public key. Some implementations send only the
        // 32-byte key.
        const DERP_MAGIC: &[u8] = b"DERP\xf0\x9f\x94\x91";
        let key_bytes: &[u8] = if server_key_frame.payload.starts_with(DERP_MAGIC) {
            &server_key_frame.payload[DERP_MAGIC.len()..]
        } else {
            &server_key_frame.payload[..]
        };
        if key_bytes.len() != 32 {
            return Err(Error::Derp(format!(
                "ServerKey must yield 32 bytes after magic, got {}",
                key_bytes.len()
            )));
        }
        let mut server_public = [0u8; 32];
        server_public.copy_from_slice(key_bytes);

        // The DERP relay needs to know our LONG-TERM node public key so it
        // can route inbound packets addressed to us. Tailscale's protocol
        // sends the node public key as the first 32 bytes of the ClientInfo
        // frame, then a NaCl-sealed JSON blob signed with the node's
        // long-term private key (so the server can verify ownership).
        let node_secret_bytes: [u8; 32] = node_keys.node_private.to_bytes();
        let node_public_bytes: [u8; 32] = *node_keys.node_public.as_bytes();
        let node_secret = SecretKey::from(node_secret_bytes);
        let node_public_nacl = PublicKey::from(node_public_bytes);

        // Build client info JSON
        let client_info = serde_json::json!({"version": 2});
        let client_info_bytes = serde_json::to_vec(&client_info)
            .map_err(|e| Error::Derp(format!("failed to serialize client info: {e}")))?;

        // Seal with crypto_box: encrypt with OUR long-term private + server public.
        let server_pk = PublicKey::from(server_public);
        let salsa_box = SalsaBox::new(&server_pk, &node_secret);
        let nonce = SalsaBox::generate_nonce(&mut OsRng);
        let sealed = salsa_box
            .encrypt(&nonce, client_info_bytes.as_slice())
            .map_err(|e| Error::Derp(format!("failed to seal client info: {e}")))?;

        // ClientInfo frame: 32-byte long-term public key + nonce + sealed box
        let mut client_info_payload = BytesMut::new();
        client_info_payload.extend_from_slice(node_public_nacl.as_bytes());
        client_info_payload.extend_from_slice(&nonce);
        client_info_payload.extend_from_slice(&sealed);

        framed
            .send(DerpFrame {
                frame_type: FRAME_CLIENT_INFO,
                payload: client_info_payload,
            })
            .await
            .map_err(|e| Error::Derp(format!("failed to send ClientInfo: {e}")))?;

        // Read ServerInfo frame (sealed box with JSON)
        let server_info_frame = framed
            .next()
            .await
            .ok_or_else(|| Error::Derp("connection closed before ServerInfo".into()))?
            .map_err(|e| Error::Derp(format!("failed to read ServerInfo frame: {e}")))?;

        if server_info_frame.frame_type != FRAME_SERVER_INFO {
            return Err(Error::Derp(format!(
                "expected ServerInfo frame (0x03), got 0x{:02x}",
                server_info_frame.frame_type
            )));
        }

        // ServerInfo is nonce + sealed JSON; we decrypt it but don't need the content
        let si_payload = &server_info_frame.payload;
        if si_payload.len() < 24 {
            return Err(Error::Derp("ServerInfo payload too short for nonce".into()));
        }
        let si_nonce = crypto_box::Nonce::from_slice(&si_payload[..24]);
        let _server_info_json = salsa_box
            .decrypt(si_nonce, &si_payload[24..])
            .map_err(|e| Error::Derp(format!("failed to decrypt ServerInfo: {e}")))?;

        // Send NotePreferred (1 byte: 0x01 = true = this is our preferred DERP)
        framed
            .send(DerpFrame {
                frame_type: FRAME_NOTE_PREFERRED,
                payload: BytesMut::from(&[0x01u8][..]),
            })
            .await
            .map_err(|e| Error::Derp(format!("failed to send NotePreferred: {e}")))?;

        Ok(DerpClient {
            inner: framed,
            server_public,
        })
    }

    /// The DERP server's public key.
    pub fn server_public(&self) -> &[u8; 32] {
        &self.server_public
    }

    /// Send a WireGuard packet to a peer via the relay.
    pub async fn send_packet(&mut self, dest_key: &[u8; 32], payload: &[u8]) -> crate::Result<()> {
        let mut frame_payload = BytesMut::with_capacity(32 + payload.len());
        frame_payload.put_slice(dest_key);
        frame_payload.put_slice(payload);

        self.inner
            .send(DerpFrame {
                frame_type: FRAME_SEND_PACKET,
                payload: frame_payload,
            })
            .await
            .map_err(|e| Error::Derp(format!("failed to send packet: {e}")))?;
        Ok(())
    }

    /// Receive a WireGuard packet from a peer via the relay.
    /// Returns (source_key, payload).
    pub async fn recv_packet(&mut self) -> crate::Result<([u8; 32], Vec<u8>)> {
        loop {
            let frame = self
                .inner
                .next()
                .await
                .ok_or(Error::ConnectionClosed)?
                .map_err(|e| Error::Derp(format!("failed to read frame: {e}")))?;

            tracing::trace!(
                "DERP frame in: type=0x{:02x} len={}",
                frame.frame_type,
                frame.payload.len()
            );

            match frame.frame_type {
                FRAME_RECV_PACKET => {
                    if frame.payload.len() < 32 {
                        return Err(Error::Derp(format!(
                            "RecvPacket payload too short: {} bytes",
                            frame.payload.len()
                        )));
                    }
                    let mut source_key = [0u8; 32];
                    source_key.copy_from_slice(&frame.payload[..32]);
                    let data = frame.payload[32..].to_vec();
                    return Ok((source_key, data));
                }
                FRAME_KEEP_ALIVE | FRAME_PEER_GONE | FRAME_PEER_PRESENT | FRAME_HEALTH => {
                    // Skip non-data frames
                    continue;
                }
                FRAME_PING => {
                    // Auto-respond with pong
                    if frame.payload.len() >= 8 {
                        let mut data = [0u8; 8];
                        data.copy_from_slice(&frame.payload[..8]);
                        self.send_pong(data).await?;
                    }
                    continue;
                }
                other => {
                    tracing::debug!("ignoring DERP frame type 0x{other:02x}");
                    continue;
                }
            }
        }
    }

    /// Send a keep-alive ping.
    pub async fn send_ping(&mut self, data: [u8; 8]) -> crate::Result<()> {
        self.inner
            .send(DerpFrame {
                frame_type: FRAME_PING,
                payload: BytesMut::from(&data[..]),
            })
            .await
            .map_err(|e| Error::Derp(format!("failed to send ping: {e}")))?;
        Ok(())
    }

    /// Send a pong response.
    pub async fn send_pong(&mut self, data: [u8; 8]) -> crate::Result<()> {
        self.inner
            .send(DerpFrame {
                frame_type: FRAME_PONG,
                payload: BytesMut::from(&data[..]),
            })
            .await
            .map_err(|e| Error::Derp(format!("failed to send pong: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use crypto_box::aead::{Aead, AeadCore, OsRng};
    use crypto_box::{PublicKey, SalsaBox, SecretKey};
    use futures::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_util::codec::Framed;

    /// Run a mock DERP server that completes the handshake.
    /// Returns the server's secret key for verification.
    async fn mock_derp_server(
        listener: &TcpListener,
    ) -> crate::Result<(Framed<TcpStream, DerpFrameCodec>, [u8; 32])> {
        let (stream, _) = listener.accept().await.map_err(|e| {
            Error::Derp(format!("accept: {e}"))
        })?;

        let (reader, mut writer) = tokio::io::split(stream);
        let mut buf_reader = BufReader::new(reader);

        // Read the HTTP upgrade request
        loop {
            let mut line = String::new();
            buf_reader.read_line(&mut line).await.map_err(|e| {
                Error::Derp(format!("read: {e}"))
            })?;
            if line.trim().is_empty() {
                break;
            }
        }

        // Send 101 response
        writer
            .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: DERP\r\n\r\n")
            .await
            .map_err(|e| Error::Derp(format!("write: {e}")))?;

        // Reassemble
        let reader = buf_reader.into_inner();
        let stream = reader.unsplit(writer);
        let mut framed = Framed::new(stream, DerpFrameCodec);

        // Generate server key
        let server_secret = SecretKey::generate(&mut OsRng);
        let server_public = server_secret.public_key();
        let server_pub_bytes: [u8; 32] = server_public.as_bytes().clone();

        // Send ServerKey frame
        framed
            .send(DerpFrame {
                frame_type: FRAME_SERVER_KEY,
                payload: BytesMut::from(server_public.as_bytes().as_slice()),
            })
            .await
            .map_err(|e| Error::Derp(format!("send ServerKey: {e}")))?;

        // Read ClientInfo frame
        let client_info_frame = framed
            .next()
            .await
            .ok_or_else(|| Error::Derp("no ClientInfo".into()))?
            .map_err(|e| Error::Derp(format!("read ClientInfo: {e}")))?;

        assert_eq!(client_info_frame.frame_type, FRAME_CLIENT_INFO);
        let ci = &client_info_frame.payload;
        assert!(ci.len() >= 32 + 24, "ClientInfo too short");

        // Extract client's ephemeral public key
        let client_pub = PublicKey::from_slice(&ci[..32])
            .map_err(|e| Error::Derp(format!("bad client pubkey: {e}")))?;
        let nonce = crypto_box::Nonce::from_slice(&ci[32..56]);
        let sealed = &ci[56..];

        // Decrypt client info
        let salsa = SalsaBox::new(&client_pub, &server_secret);
        let client_info_json = salsa
            .decrypt(nonce, sealed)
            .map_err(|e| Error::Derp(format!("decrypt client info: {e}")))?;
        let _: serde_json::Value = serde_json::from_slice(&client_info_json)?;

        // Send ServerInfo frame (nonce + sealed JSON)
        let server_info = serde_json::json!({"version": 2});
        let server_info_bytes = serde_json::to_vec(&server_info).unwrap();
        let si_nonce = SalsaBox::generate_nonce(&mut OsRng);
        let si_sealed = salsa
            .encrypt(&si_nonce, server_info_bytes.as_slice())
            .map_err(|e| Error::Derp(format!("encrypt server info: {e}")))?;

        let mut si_payload = BytesMut::new();
        si_payload.extend_from_slice(&si_nonce);
        si_payload.extend_from_slice(&si_sealed);

        framed
            .send(DerpFrame {
                frame_type: FRAME_SERVER_INFO,
                payload: si_payload,
            })
            .await
            .map_err(|e| Error::Derp(format!("send ServerInfo: {e}")))?;

        // Read NotePreferred frame
        let note_frame = framed
            .next()
            .await
            .ok_or_else(|| Error::Derp("no NotePreferred".into()))?
            .map_err(|e| Error::Derp(format!("read NotePreferred: {e}")))?;
        assert_eq!(note_frame.frame_type, FRAME_NOTE_PREFERRED);
        assert_eq!(&note_frame.payload[..], &[0x01]);

        Ok((framed, server_pub_bytes))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires multi-step mock server; covered by docker-compose integration tests"]
    async fn test_derp_handshake_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let node_keys = crate::keys::NodeKeys::generate();

        let server_handle = tokio::spawn(async move { mock_derp_server(&listener).await });

        let client = DerpClient::connect(&addr.to_string(), &node_keys)
            .await
            .unwrap();

        let (_, server_pub) = server_handle.await.unwrap().unwrap();
        assert_eq!(client.server_public(), &server_pub);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires multi-step mock server; covered by docker-compose integration tests"]
    async fn test_send_recv_packet() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let node_keys = crate::keys::NodeKeys::generate();

        // Run the full test as a spawned task so we can add a timeout
        let test_body = async move {
            let server_handle =
                tokio::spawn(async move { mock_derp_server(&listener).await });

            let mut client = DerpClient::connect(&addr.to_string(), &node_keys)
                .await
                .unwrap();

            let (mut server_framed, _) = server_handle.await.unwrap().unwrap();

            // Client sends a packet
            let dest_key = [0xAA; 32];
            client
                .send_packet(&dest_key, b"test wireguard packet")
                .await
                .unwrap();

            // Server reads the SendPacket, replies with RecvPacket,
            // while client concurrently waits for RecvPacket.
            let server_task = tokio::spawn(async move {
                let frame = server_framed.next().await.unwrap().unwrap();
                assert_eq!(frame.frame_type, FRAME_SEND_PACKET);
                assert_eq!(&frame.payload[..32], &[0xAA; 32]);
                assert_eq!(&frame.payload[32..], b"test wireguard packet");

                let source_key = [0xBB; 32];
                let mut recv_payload = BytesMut::new();
                recv_payload.put_slice(&source_key);
                recv_payload.put_slice(b"reply packet");
                server_framed
                    .send(DerpFrame {
                        frame_type: FRAME_RECV_PACKET,
                        payload: recv_payload,
                    })
                    .await
                    .unwrap();
            });

            let (server_res, client_res) =
                tokio::join!(server_task, client.recv_packet());

            server_res.unwrap();
            let (src, data) = client_res.unwrap();
            assert_eq!(src, [0xBB; 32]);
            assert_eq!(data, b"reply packet");
        };

        tokio::time::timeout(std::time::Duration::from_secs(10), test_body)
            .await
            .expect("test_send_recv_packet timed out after 5 seconds");
    }
}
