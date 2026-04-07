//! Tailscale controlbase handshake implementation.
//!
//! This implements the custom Noise IK variant used by the Tailscale/Headscale
//! control protocol. It is NOT compatible with standard Noise libraries like `snow`.

use base64::Engine;
use blake2::Digest;
use chacha20poly1305::{AeadInPlace, ChaCha20Poly1305, KeyInit, Nonce, Tag};
use hkdf::Hkdf;
use hmac::SimpleHmac;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use x25519_dalek::{PublicKey, StaticSecret};

type HkdfBlake2s = Hkdf<blake2::Blake2s256, SimpleHmac<blake2::Blake2s256>>;

const PROTOCOL_NAME: &[u8] = b"Noise_IK_25519_ChaChaPoly_BLAKE2s";
const PROTOCOL_VERSION_PREFIX: &[u8] = b"Tailscale Control Protocol v";
const PROTOCOL_VERSION: u16 = 49;

// Message types
const MSG_TYPE_INITIATION: u8 = 1;
const MSG_TYPE_RESPONSE: u8 = 2;
const MSG_TYPE_ERROR: u8 = 3;
pub(crate) const MSG_TYPE_RECORD: u8 = 4;

// Sizes
const TAG_SIZE: usize = 16;

pub(crate) struct HandshakeResult {
    pub tx_cipher: ChaCha20Poly1305, // client -> server
    pub rx_cipher: ChaCha20Poly1305, // server -> client
    pub handshake_hash: [u8; 32],
    pub protocol_version: u16,
    /// Leftover bytes from the TCP buffer after the handshake response.
    /// These are the start of the first Noise transport record.
    pub leftover: Vec<u8>,
}

struct SymmetricState {
    h: [u8; 32],  // hash
    ck: [u8; 32], // chaining key
}

impl SymmetricState {
    fn initialize() -> Self {
        let h: [u8; 32] = blake2::Blake2s256::digest(PROTOCOL_NAME).into();
        Self { h, ck: h }
    }

    fn mix_hash(&mut self, data: &[u8]) {
        let mut hasher = blake2::Blake2s256::new();
        hasher.update(&self.h);
        hasher.update(data);
        self.h = hasher.finalize().into();
    }

    fn mix_dh(
        &mut self,
        priv_key: &StaticSecret,
        pub_key: &PublicKey,
    ) -> crate::Result<ChaCha20Poly1305> {
        let shared = priv_key.diffie_hellman(pub_key);
        let hk = HkdfBlake2s::new(Some(&self.ck), shared.as_bytes());
        let mut output = [0u8; 64];
        hk.expand(&[], &mut output)
            .map_err(|e| crate::Error::Noise(format!("HKDF expand: {e}")))?;
        self.ck.copy_from_slice(&output[..32]);
        let key: [u8; 32] = output[32..64].try_into().unwrap();
        ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|e| crate::Error::Noise(format!("AEAD key: {e}")))
    }

    fn encrypt_and_hash(&mut self, cipher: ChaCha20Poly1305, plaintext: &[u8]) -> Vec<u8> {
        let nonce = Nonce::default(); // all zeros
        let mut ciphertext = plaintext.to_vec();
        let tag = cipher
            .encrypt_in_place_detached(&nonce, &self.h, &mut ciphertext)
            .expect("encryption failed");
        ciphertext.extend_from_slice(tag.as_slice());
        self.mix_hash(&ciphertext);
        ciphertext
    }

    fn decrypt_and_hash(
        &mut self,
        cipher: ChaCha20Poly1305,
        ciphertext: &[u8],
    ) -> crate::Result<Vec<u8>> {
        if ciphertext.len() < TAG_SIZE {
            return Err(crate::Error::Noise("ciphertext too short".into()));
        }
        let nonce = Nonce::default();
        let (ct, tag_bytes) = ciphertext.split_at(ciphertext.len() - TAG_SIZE);
        let tag = Tag::from_slice(tag_bytes);
        let mut plaintext = ct.to_vec();
        cipher
            .decrypt_in_place_detached(&nonce, &self.h, &mut plaintext, tag)
            .map_err(|_| crate::Error::Noise("AEAD decryption failed".into()))?;
        self.mix_hash(ciphertext);
        Ok(plaintext)
    }

    fn split(self) -> crate::Result<(ChaCha20Poly1305, ChaCha20Poly1305)> {
        let hk = HkdfBlake2s::new(Some(&self.ck), &[]);
        let mut output = [0u8; 64];
        hk.expand(&[], &mut output)
            .map_err(|e| crate::Error::Noise(format!("HKDF split: {e}")))?;
        let k1: [u8; 32] = output[..32].try_into().unwrap();
        let k2: [u8; 32] = output[32..64].try_into().unwrap();
        let c1 = ChaCha20Poly1305::new_from_slice(&k1)
            .map_err(|e| crate::Error::Noise(format!("c1 key: {e}")))?;
        let c2 = ChaCha20Poly1305::new_from_slice(&k2)
            .map_err(|e| crate::Error::Noise(format!("c2 key: {e}")))?;
        Ok((c1, c2))
    }
}

fn protocol_version_prologue(version: u16) -> Vec<u8> {
    let mut p = PROTOCOL_VERSION_PREFIX.to_vec();
    p.extend_from_slice(version.to_string().as_bytes());
    p
}

/// Perform the TS2021 HTTP upgrade and controlbase Noise IK handshake.
///
/// Returns a [`HandshakeResult`] containing the transport ciphers.
pub(crate) async fn perform_handshake(
    stream: &mut TcpStream,
    machine_private: &StaticSecret,
    machine_public: &PublicKey,
    server_public: &PublicKey,
) -> crate::Result<HandshakeResult> {
    let mut s = SymmetricState::initialize();

    // Prologue
    s.mix_hash(&protocol_version_prologue(PROTOCOL_VERSION));

    // <- s (mix in known server static key)
    s.mix_hash(server_public.as_bytes());

    // -> e, es, s, ss
    let ephemeral_secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);

    // Build initiation message (101 bytes)
    let mut init = vec![0u8; 101];
    // Header
    init[0..2].copy_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    init[2] = MSG_TYPE_INITIATION;
    init[3..5].copy_from_slice(&96u16.to_be_bytes());
    // Payload
    init[5..37].copy_from_slice(ephemeral_public.as_bytes());
    s.mix_hash(ephemeral_public.as_bytes());

    let cipher_es = s.mix_dh(&ephemeral_secret, server_public)?;
    let encrypted_machine_pub = s.encrypt_and_hash(cipher_es, machine_public.as_bytes());
    init[37..85].copy_from_slice(&encrypted_machine_pub);

    let cipher_ss = s.mix_dh(machine_private, server_public)?;
    let tag = s.encrypt_and_hash(cipher_ss, &[]);
    init[85..101].copy_from_slice(&tag);

    // Base64-encode for HTTP header
    let init_b64 = base64::engine::general_purpose::STANDARD.encode(&init);

    // Send HTTP upgrade
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "localhost".into());
    let upgrade_req = format!(
        "POST /ts2021 HTTP/1.1\r\n\
         Host: {peer}\r\n\
         Upgrade: tailscale-control-protocol\r\n\
         Connection: upgrade\r\n\
         X-Tailscale-Handshake: {init_b64}\r\n\
         \r\n"
    );
    stream
        .write_all(upgrade_req.as_bytes())
        .await
        .map_err(|e| crate::Error::Noise(format!("write upgrade: {e}")))?;

    // Read HTTP 101 response
    let mut resp_buf = vec![0u8; 8192];
    let mut total = 0;
    loop {
        let n = stream
            .read(&mut resp_buf[total..])
            .await
            .map_err(|e| crate::Error::Noise(format!("read response: {e}")))?;
        if n == 0 {
            return Err(crate::Error::ConnectionClosed);
        }
        total += n;
        if resp_buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = resp_buf[..total]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .ok_or_else(|| crate::Error::Noise("no header boundary".into()))?;

    let headers = String::from_utf8_lossy(&resp_buf[..header_end]);
    if !headers.contains("101") {
        let body = String::from_utf8_lossy(&resp_buf[header_end..total]);
        return Err(crate::Error::Noise(format!(
            "expected 101, got: {headers}\nBody: {body}"
        )));
    }

    // Read server response message (51 bytes: 3 header + 48 payload)
    // Some bytes may already be in resp_buf after the HTTP headers.
    let mut resp_data = resp_buf[header_end..total].to_vec();
    while resp_data.len() < 51 {
        let mut tmp = vec![0u8; 51 - resp_data.len()];
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|e| crate::Error::Noise(format!("read response msg: {e}")))?;
        if n == 0 {
            return Err(crate::Error::ConnectionClosed);
        }
        resp_data.extend_from_slice(&tmp[..n]);
    }

    // Parse response
    if resp_data[0] == MSG_TYPE_ERROR {
        let err_len = u16::from_be_bytes([resp_data[1], resp_data[2]]) as usize;
        let err_msg =
            String::from_utf8_lossy(&resp_data[3..3 + err_len.min(resp_data.len() - 3)]);
        return Err(crate::Error::Noise(format!("server error: {err_msg}")));
    }
    if resp_data[0] != MSG_TYPE_RESPONSE {
        return Err(crate::Error::Noise(format!(
            "unexpected response type {}",
            resp_data[0]
        )));
    }

    let server_ephemeral_bytes: [u8; 32] = resp_data[3..35].try_into().unwrap();
    let server_ephemeral = PublicKey::from(server_ephemeral_bytes);
    let resp_tag = &resp_data[35..51];

    // <- e, ee, se
    s.mix_hash(server_ephemeral.as_bytes());
    let _cipher_ee = s.mix_dh(&ephemeral_secret, &server_ephemeral)?;
    let cipher_se = s.mix_dh(machine_private, &server_ephemeral)?;
    s.decrypt_and_hash(cipher_se, resp_tag)?;

    // Split into transport ciphers
    let h = s.h;
    let (c1, c2) = s.split()?;

    tracing::debug!("controlbase handshake complete");

    // Any bytes in resp_data beyond the 51-byte response are the start
    // of the first Noise transport record (early payload).
    let leftover = if resp_data.len() > 51 {
        resp_data[51..].to_vec()
    } else {
        vec![]
    };

    Ok(HandshakeResult {
        tx_cipher: c1,
        rx_cipher: c2,
        handshake_hash: h,
        protocol_version: PROTOCOL_VERSION,
        leftover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Mock server that implements the controlbase server-side handshake.
    async fn mock_controlbase_server(
        mut sock: TcpStream,
        server_private: StaticSecret,
        server_public: PublicKey,
    ) {
        // Read the full HTTP request.
        let mut buf = vec![0u8; 8192];
        let mut total = 0;
        loop {
            let n = sock.read(&mut buf[total..]).await.unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        // Extract the X-Tailscale-Handshake header.
        let req_str = String::from_utf8_lossy(&buf[..total]);
        let init_b64 = req_str
            .lines()
            .find(|l| l.starts_with("X-Tailscale-Handshake:"))
            .map(|l| l.trim_start_matches("X-Tailscale-Handshake:").trim())
            .expect("missing X-Tailscale-Handshake header");
        let init_msg = base64::engine::general_purpose::STANDARD
            .decode(init_b64)
            .expect("bad base64");

        assert_eq!(init_msg.len(), 101, "initiation message should be 101 bytes");

        // Parse initiation
        let _version = u16::from_be_bytes([init_msg[0], init_msg[1]]);
        assert_eq!(init_msg[2], MSG_TYPE_INITIATION);
        let _payload_len = u16::from_be_bytes([init_msg[3], init_msg[4]]);

        let client_ephemeral_bytes: [u8; 32] = init_msg[5..37].try_into().unwrap();
        let client_ephemeral = PublicKey::from(client_ephemeral_bytes);
        let encrypted_machine_pub = &init_msg[37..85];
        let init_tag = &init_msg[85..101];

        // Server-side symmetric state
        let mut s = SymmetricState::initialize();
        s.mix_hash(&protocol_version_prologue(PROTOCOL_VERSION));
        s.mix_hash(server_public.as_bytes());

        // Process -> e
        s.mix_hash(client_ephemeral.as_bytes());

        // -> es: MixDH(server_private, client_ephemeral)
        let cipher_es = s.mix_dh(&server_private, &client_ephemeral).unwrap();

        // -> s: DecryptAndHash to get client machine public key
        let client_machine_pub_bytes = s.decrypt_and_hash(cipher_es, encrypted_machine_pub).unwrap();
        assert_eq!(client_machine_pub_bytes.len(), 32);
        let client_machine_pub = PublicKey::from(<[u8; 32]>::try_from(&client_machine_pub_bytes[..]).unwrap());

        // -> ss: MixDH(server_private, client_machine_pub)
        let cipher_ss = s.mix_dh(&server_private, &client_machine_pub).unwrap();
        s.decrypt_and_hash(cipher_ss, init_tag).unwrap();

        // Send 101 response.
        sock.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\n\
              Connection: Upgrade\r\n\
              Upgrade: tailscale-control-protocol\r\n\
              \r\n",
        )
        .await
        .unwrap();

        // <- e (server ephemeral)
        let server_ephemeral_secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let server_ephemeral_public = PublicKey::from(&server_ephemeral_secret);

        s.mix_hash(server_ephemeral_public.as_bytes());

        // <- ee: MixDH(server_ephemeral_private, client_ephemeral)
        let _cipher_ee = s.mix_dh(&server_ephemeral_secret, &client_ephemeral).unwrap();

        // <- se: MixDH(server_ephemeral_private, client_machine_pub)
        let cipher_se = s.mix_dh(&server_ephemeral_secret, &client_machine_pub).unwrap();
        let tag = s.encrypt_and_hash(cipher_se, &[]);

        // Build response message (51 bytes)
        let mut resp = vec![0u8; 51];
        resp[0] = MSG_TYPE_RESPONSE;
        resp[1..3].copy_from_slice(&48u16.to_be_bytes());
        resp[3..35].copy_from_slice(server_ephemeral_public.as_bytes());
        resp[35..51].copy_from_slice(&tag);

        sock.write_all(&resp).await.unwrap();

        // Split for transport
        let (_c1, _c2) = s.split().unwrap();
    }

    #[tokio::test]
    async fn handshake_with_mock_server() {
        // Generate server keys.
        let server_private = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let server_public = PublicKey::from(&server_private);

        // Generate client keys.
        let machine_private = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let machine_public = PublicKey::from(&machine_private);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let sp = server_public;
        let spriv = server_private.clone();

        // Server task.
        let server_handle = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            mock_controlbase_server(sock, spriv, sp).await;
        });

        // Client.
        let mut client_stream = TcpStream::connect(addr).await.unwrap();
        let result = perform_handshake(
            &mut client_stream,
            &machine_private,
            &machine_public,
            &server_public,
        )
        .await
        .unwrap();

        // Verify we got transport ciphers and can encrypt.
        let nonce = Nonce::default();
        let mut data = b"hello".to_vec();
        result
            .tx_cipher
            .encrypt_in_place_detached(&nonce, &[], &mut data)
            .unwrap();
        // Ciphertext should differ from plaintext.
        assert_ne!(&data[..], b"hello");

        assert_eq!(result.protocol_version, 49);

        server_handle.await.unwrap();
    }
}
