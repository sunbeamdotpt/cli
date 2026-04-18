//! Tailscale-compatible disco protocol: NaCl-box encrypted peer discovery
//! messages (Ping, Pong, CallMeMaybe).

pub mod packet;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crypto_box::aead::{Aead, AeadCore, OsRng};
use crypto_box::SalsaBox;

/// Disco magic bytes: UTF-8 encoding of "TS💬".
pub const MAGIC: [u8; 6] = [0x54, 0x53, 0xF0, 0x9F, 0x92, 0xAC];

/// Total header length: magic (6) + sender disco pubkey (32) + nonce (24).
pub const HEADER_LEN: usize = 6 + 32 + 24; // 62

/// Disco message types.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Ping(Ping),
    Pong(Pong),
    CallMeMaybe(CallMeMaybe),
}

/// Disco ping — used to probe connectivity and measure RTT.
#[derive(Debug, Clone, PartialEq)]
pub struct Ping {
    pub tx_id: [u8; 12],
    /// If present, the sender's node key (for identification).
    pub node_key: Option<[u8; 32]>,
    /// Number of zero-padding bytes appended (for MTU probing).
    pub padding: usize,
}

/// Disco pong — reply to a ping, echoing back the observed source address.
#[derive(Debug, Clone, PartialEq)]
pub struct Pong {
    pub tx_id: [u8; 12],
    /// The source address the pinger was observed at.
    pub src: SocketAddr,
}

/// CallMeMaybe — advertise candidate endpoints for direct connectivity.
#[derive(Debug, Clone, PartialEq)]
pub struct CallMeMaybe {
    pub endpoints: Vec<SocketAddr>,
}

// Wire type tags
const TYPE_PING: u8 = 0x01;
const TYPE_PONG: u8 = 0x02;
const TYPE_CALL_ME_MAYBE: u8 = 0x03;

// Wire version
const VERSION: u8 = 0;

/// Encode a disco message to its inner (unencrypted) wire format.
pub fn marshal(msg: &Message) -> Vec<u8> {
    match msg {
        Message::Ping(p) => {
            let mut buf = Vec::with_capacity(2 + 12 + 32 + p.padding);
            buf.push(TYPE_PING);
            buf.push(VERSION);
            buf.extend_from_slice(&p.tx_id);
            if let Some(ref nk) = p.node_key {
                buf.extend_from_slice(nk);
            }
            buf.extend(std::iter::repeat_n(0u8, p.padding));
            buf
        }
        Message::Pong(p) => {
            let mut buf = Vec::with_capacity(2 + 12 + 16 + 2);
            buf.push(TYPE_PONG);
            buf.push(VERSION);
            buf.extend_from_slice(&p.tx_id);
            buf.extend_from_slice(&addr_to_v6_mapped(&p.src));
            buf.extend_from_slice(&p.src.port().to_be_bytes());
            buf
        }
        Message::CallMeMaybe(c) => {
            let mut buf = Vec::with_capacity(2 + c.endpoints.len() * 18);
            buf.push(TYPE_CALL_ME_MAYBE);
            buf.push(VERSION);
            for ep in &c.endpoints {
                buf.extend_from_slice(&addr_to_v6_mapped(ep));
                buf.extend_from_slice(&ep.port().to_be_bytes());
            }
            buf
        }
    }
}

/// Decode an inner (unencrypted) disco message payload.
pub fn unmarshal(data: &[u8]) -> Option<Message> {
    if data.len() < 2 {
        return None;
    }
    let msg_type = data[0];
    let _version = data[1];
    let body = &data[2..];

    match msg_type {
        TYPE_PING => {
            if body.len() < 12 {
                return None;
            }
            let mut tx_id = [0u8; 12];
            tx_id.copy_from_slice(&body[..12]);
            let rest = &body[12..];
            let (node_key, padding_start) = if rest.len() >= 32 {
                let mut nk = [0u8; 32];
                nk.copy_from_slice(&rest[..32]);
                (Some(nk), 32)
            } else {
                (None, 0)
            };
            let padding = rest.len() - padding_start;
            Some(Message::Ping(Ping {
                tx_id,
                node_key,
                padding,
            }))
        }
        TYPE_PONG => {
            if body.len() < 12 + 16 + 2 {
                return None;
            }
            let mut tx_id = [0u8; 12];
            tx_id.copy_from_slice(&body[..12]);
            let ip_bytes = &body[12..28];
            let port = u16::from_be_bytes([body[28], body[29]]);
            let addr = v6_mapped_to_addr(ip_bytes, port);
            Some(Message::Pong(Pong { tx_id, src: addr }))
        }
        TYPE_CALL_ME_MAYBE => {
            let mut endpoints = Vec::new();
            let mut pos = 0;
            while pos + 18 <= body.len() {
                let ip_bytes = &body[pos..pos + 16];
                let port = u16::from_be_bytes([body[pos + 16], body[pos + 17]]);
                endpoints.push(v6_mapped_to_addr(ip_bytes, port));
                pos += 18;
            }
            Some(Message::CallMeMaybe(CallMeMaybe { endpoints }))
        }
        _ => None,
    }
}

/// Encrypt and frame a disco message for the wire.
///
/// Returns `MAGIC || my_disco_pub || nonce || NaCl_box(marshal(msg))`.
pub fn seal(msg: &Message, my_disco_pub: &[u8; 32], shared: &SalsaBox) -> Vec<u8> {
    let plaintext = marshal(msg);
    let nonce = SalsaBox::generate_nonce(&mut OsRng);
    let ciphertext = shared
        .encrypt(&nonce, plaintext.as_ref())
        .expect("disco seal: encryption should not fail");

    let mut pkt = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    pkt.extend_from_slice(&MAGIC);
    pkt.extend_from_slice(my_disco_pub);
    pkt.extend_from_slice(&nonce);
    pkt.extend_from_slice(&ciphertext);
    pkt
}

/// Decrypt and parse an incoming disco packet.
///
/// `shared_keys` maps sender disco public keys to precomputed `SalsaBox`es.
/// Returns the decoded message and the sender's disco public key.
pub fn open(
    packet: &[u8],
    shared_keys: &HashMap<[u8; 32], SalsaBox>,
) -> Option<(Message, [u8; 32])> {
    if packet.len() < HEADER_LEN {
        return None;
    }
    if packet[..6] != MAGIC {
        return None;
    }

    let mut sender_pub = [0u8; 32];
    sender_pub.copy_from_slice(&packet[6..38]);

    let shared = shared_keys.get(&sender_pub)?;

    let nonce = crypto_box::Nonce::from_slice(&packet[38..62]);
    let ciphertext = &packet[62..];

    let plaintext = shared.decrypt(nonce, ciphertext).ok()?;
    let msg = unmarshal(&plaintext)?;

    Some((msg, sender_pub))
}

// ---------- IPv6-mapped address helpers ----------

fn addr_to_v6_mapped(addr: &SocketAddr) -> [u8; 16] {
    match addr.ip() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, o[0], o[1], o[2], o[3],
            ]
        }
        IpAddr::V6(v6) => v6.octets(),
    }
}

fn v6_mapped_to_addr(bytes: &[u8], port: u16) -> SocketAddr {
    // Check for IPv4-mapped IPv6: ::ffff:a.b.c.d
    if bytes[..10] == [0; 10] && bytes[10] == 0xff && bytes[11] == 0xff {
        SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15])),
            port,
        )
    } else {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(bytes);
        SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_box::SecretKey;

    #[test]
    fn test_marshal_unmarshal_ping() {
        // Without node_key
        let ping = Message::Ping(Ping {
            tx_id: [1; 12],
            node_key: None,
            padding: 0,
        });
        let data = marshal(&ping);
        let parsed = unmarshal(&data).unwrap();
        assert_eq!(parsed, ping);

        // With node_key
        let ping_nk = Message::Ping(Ping {
            tx_id: [2; 12],
            node_key: Some([0xAA; 32]),
            padding: 0,
        });
        let data = marshal(&ping_nk);
        let parsed = unmarshal(&data).unwrap();
        assert_eq!(parsed, ping_nk);

        // With padding
        let ping_pad = Message::Ping(Ping {
            tx_id: [3; 12],
            node_key: Some([0xBB; 32]),
            padding: 100,
        });
        let data = marshal(&ping_pad);
        let parsed = unmarshal(&data).unwrap();
        // Padding bytes are parsed as padding count
        if let Message::Ping(p) = &parsed {
            assert_eq!(p.tx_id, [3; 12]);
            assert_eq!(p.node_key, Some([0xBB; 32]));
            assert_eq!(p.padding, 100);
        } else {
            panic!("expected Ping");
        }
    }

    #[test]
    fn test_marshal_unmarshal_pong_v4() {
        let addr: SocketAddr = "192.168.1.1:12345".parse().unwrap();
        let pong = Message::Pong(Pong {
            tx_id: [5; 12],
            src: addr,
        });
        let data = marshal(&pong);
        let parsed = unmarshal(&data).unwrap();
        assert_eq!(parsed, pong);
    }

    #[test]
    fn test_marshal_unmarshal_pong_v6() {
        let addr: SocketAddr = "[2001:db8::1]:9999".parse().unwrap();
        let pong = Message::Pong(Pong {
            tx_id: [6; 12],
            src: addr,
        });
        let data = marshal(&pong);
        let parsed = unmarshal(&data).unwrap();
        assert_eq!(parsed, pong);
    }

    #[test]
    fn test_marshal_unmarshal_call_me_maybe() {
        let cmm = Message::CallMeMaybe(CallMeMaybe {
            endpoints: vec![
                "10.0.0.1:1234".parse().unwrap(),
                "[::1]:5678".parse().unwrap(),
                "172.16.0.1:9012".parse().unwrap(),
            ],
        });
        let data = marshal(&cmm);
        let parsed = unmarshal(&data).unwrap();
        assert_eq!(parsed, cmm);
    }

    #[test]
    fn test_seal_open_round_trip() {
        let sk_a = SecretKey::generate(&mut OsRng);
        let pk_a = sk_a.public_key();
        let sk_b = SecretKey::generate(&mut OsRng);
        let pk_b = sk_b.public_key();

        // A seals for B
        let shared_a = SalsaBox::new(&pk_b, &sk_a);
        // B opens from A
        let shared_b = SalsaBox::new(&pk_a, &sk_b);

        let msg = Message::Ping(Ping {
            tx_id: [42; 12],
            node_key: None,
            padding: 0,
        });

        let pub_a_bytes: [u8; 32] = *pk_a.as_bytes();
        let sealed = seal(&msg, &pub_a_bytes, &shared_a);

        let mut keys = HashMap::new();
        keys.insert(pub_a_bytes, shared_b);

        let (opened, sender) = open(&sealed, &keys).unwrap();
        assert_eq!(opened, msg);
        assert_eq!(sender, pub_a_bytes);
    }

    #[test]
    fn test_open_wrong_key_fails() {
        let sk_a = SecretKey::generate(&mut OsRng);
        let pk_a = sk_a.public_key();
        let sk_b = SecretKey::generate(&mut OsRng);
        let pk_b = sk_b.public_key();
        let sk_c = SecretKey::generate(&mut OsRng);
        let pk_c = sk_c.public_key();

        // A seals for B
        let shared_a = SalsaBox::new(&pk_b, &sk_a);

        let msg = Message::Pong(Pong {
            tx_id: [7; 12],
            src: "1.2.3.4:80".parse().unwrap(),
        });

        let pub_a_bytes: [u8; 32] = *pk_a.as_bytes();
        let sealed = seal(&msg, &pub_a_bytes, &shared_a);

        // C tries to open with wrong shared key
        let wrong_shared = SalsaBox::new(&pk_a, &sk_c);
        let mut keys = HashMap::new();
        keys.insert(pub_a_bytes, wrong_shared);

        assert!(open(&sealed, &keys).is_none());

        let _ = (pk_c,); // suppress unused
    }

    #[test]
    fn test_magic_constant() {
        assert_eq!(&MAGIC, "TS\u{1F4AC}".as_bytes());
    }
}
