//! RFC 5389 STUN binding request/response codec for NAT detection.

pub mod netcheck;

/// STUN transaction ID (12 bytes following the magic cookie).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxId(pub [u8; 12]);

impl TxId {
    /// Generate a random transaction ID.
    pub fn random() -> Self {
        let mut buf = [0u8; 12];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut buf);
        Self(buf)
    }
}

/// RFC 5389 magic cookie.
pub const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

/// Encode a STUN binding request with SOFTWARE and FINGERPRINT attributes.
///
/// SOFTWARE is set to `"tailnode"` because tailscale's embedded DERP STUN
/// server (used by headscale) rejects probes with any other value — see
/// `net/stun/stun.go:ParseBindingRequest` in the tailscale repo.
pub fn request(tx_id: &TxId) -> Vec<u8> {
    // SOFTWARE attribute: type 0x8022, value "tailnode" (8 bytes, no padding)
    let software_value = b"tailnode";
    let software_padded_len = 8;
    let software_attr_len = 4 + software_padded_len; // type(2) + len(2) + value(8)

    // FINGERPRINT attribute: type 0x8028, length 4, value 4 bytes
    let fingerprint_attr_len = 4 + 4;

    let attrs_len = software_attr_len + fingerprint_attr_len;

    // Total message length in header = length of all attributes
    let mut buf = Vec::with_capacity(20 + attrs_len);

    // Header: type = 0x0001 (Binding Request)
    buf.extend_from_slice(&0x0001u16.to_be_bytes());
    // Message length (attributes only, not header)
    buf.extend_from_slice(&(attrs_len as u16).to_be_bytes());
    // Magic cookie
    buf.extend_from_slice(&MAGIC_COOKIE);
    // Transaction ID
    buf.extend_from_slice(&tx_id.0);

    // SOFTWARE attribute (0x8022). "tailnode" is 8 bytes so no padding needed.
    buf.extend_from_slice(&0x8022u16.to_be_bytes());
    buf.extend_from_slice(&(software_value.len() as u16).to_be_bytes());
    buf.extend_from_slice(software_value);

    // For FINGERPRINT, the CRC is computed over everything before the
    // FINGERPRINT attribute, but with the message length adjusted to include
    // the FINGERPRINT attribute itself.
    // Current message length in header already includes fingerprint_attr_len,
    // so the header is already correct for the CRC calculation.
    let crc = crc32(&buf) ^ 0x5354554e;

    // FINGERPRINT attribute (0x8028)
    buf.extend_from_slice(&0x8028u16.to_be_bytes());
    buf.extend_from_slice(&4u16.to_be_bytes());
    buf.extend_from_slice(&crc.to_be_bytes());

    buf
}

/// Parse a STUN binding response, extracting the XOR-MAPPED-ADDRESS.
///
/// Returns the transaction ID and the reflexive transport address on success.
pub fn parse_response(buf: &[u8]) -> Option<(TxId, std::net::SocketAddr)> {
    if buf.len() < 20 {
        return None;
    }
    // Check magic cookie
    if buf[4..8] != MAGIC_COOKIE {
        return None;
    }
    // First two bits must be 0
    if buf[0] & 0xC0 != 0 {
        return None;
    }
    // Must be a success response (0x0101)
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != 0x0101 {
        return None;
    }

    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if buf.len() < 20 + msg_len {
        return None;
    }

    let mut tx_id = [0u8; 12];
    tx_id.copy_from_slice(&buf[8..20]);

    // Walk attributes looking for XOR-MAPPED-ADDRESS (0x0020)
    let mut pos = 20;
    let end = 20 + msg_len;
    while pos + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let attr_len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        let attr_data_start = pos + 4;
        let attr_data_end = attr_data_start + attr_len;
        if attr_data_end > end {
            return None;
        }

        if attr_type == 0x0020 {
            // XOR-MAPPED-ADDRESS
            if attr_len < 4 {
                return None;
            }
            // byte 0: reserved, byte 1: family, bytes 2-3: xor'd port
            let family = buf[attr_data_start + 1];
            let xport = u16::from_be_bytes([buf[attr_data_start + 2], buf[attr_data_start + 3]]);
            let port = xport ^ 0x2112; // top 16 bits of magic cookie

            match family {
                0x01 => {
                    // IPv4: 4 bytes XOR'd with magic cookie
                    if attr_len < 8 {
                        return None;
                    }
                    let mut ip_bytes = [0u8; 4];
                    for i in 0..4 {
                        ip_bytes[i] = buf[attr_data_start + 4 + i] ^ MAGIC_COOKIE[i];
                    }
                    let addr = std::net::SocketAddr::new(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::from(ip_bytes)),
                        port,
                    );
                    return Some((TxId(tx_id), addr));
                }
                0x02 => {
                    // IPv6: 16 bytes XOR'd with magic cookie + tx_id
                    if attr_len < 20 {
                        return None;
                    }
                    let mut ip_bytes = [0u8; 16];
                    let mut xor_key = [0u8; 16];
                    xor_key[..4].copy_from_slice(&MAGIC_COOKIE);
                    xor_key[4..].copy_from_slice(&tx_id);
                    for i in 0..16 {
                        ip_bytes[i] = buf[attr_data_start + 4 + i] ^ xor_key[i];
                    }
                    let v6 = std::net::Ipv6Addr::from(ip_bytes);
                    // Unwrap IPv4-mapped-IPv6 so downstream sees a plain v4.
                    let ip = match v6.to_ipv4_mapped() {
                        Some(v4) => std::net::IpAddr::V4(v4),
                        None => std::net::IpAddr::V6(v6),
                    };
                    return Some((TxId(tx_id), std::net::SocketAddr::new(ip, port)));
                }
                _ => return None,
            }
        }

        // Advance to next attribute (padded to 4-byte boundary)
        let padded = (attr_len + 3) & !3;
        pos = attr_data_start + padded;
    }

    None
}

/// Returns `true` if `buf` looks like a STUN message.
///
/// Checks: length >= 20, bytes 4..8 == magic cookie, first two bits of byte 0
/// are zero.
pub fn is_stun(buf: &[u8]) -> bool {
    buf.len() >= 20 && buf[4..8] == MAGIC_COOKIE && (buf[0] & 0xC0 == 0)
}

// ---------- CRC32 (IEEE / ITU-T V.42) ----------

/// Compute CRC32 (ISO 3309 / ITU-T V.42) over `data`.
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut crc = i;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB88320;
                } else {
                    crc >>= 1;
                }
            }
            t[i as usize] = crc;
        }
        t
    });

    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

// ---------- helpers for building test response packets ----------

/// Build a minimal STUN success response containing an XOR-MAPPED-ADDRESS.
#[cfg(test)]
pub(crate) fn build_response(tx_id: &TxId, addr: std::net::SocketAddr) -> Vec<u8> {
    let (family, xor_addr_bytes) = match addr {
        std::net::SocketAddr::V4(v4) => {
            let octets = v4.ip().octets();
            let mut xored = [0u8; 4];
            for i in 0..4 {
                xored[i] = octets[i] ^ MAGIC_COOKIE[i];
            }
            (0x01u8, xored.to_vec())
        }
        std::net::SocketAddr::V6(v6) => {
            let octets = v6.ip().octets();
            let mut xor_key = [0u8; 16];
            xor_key[..4].copy_from_slice(&MAGIC_COOKIE);
            xor_key[4..].copy_from_slice(&tx_id.0);
            let mut xored = [0u8; 16];
            for i in 0..16 {
                xored[i] = octets[i] ^ xor_key[i];
            }
            (0x02u8, xored.to_vec())
        }
    };

    let xport = (addr.port() ^ 0x2112).to_be_bytes();
    let attr_value_len = 4 + xor_addr_bytes.len(); // reserved(1)+family(1)+port(2)+addr
    let attr_total = 4 + attr_value_len; // type(2)+len(2)+value
    let padded_value_len = (attr_value_len + 3) & !3;
    let padded_attr_total = 4 + padded_value_len;

    let msg_len = padded_attr_total;
    let mut buf = Vec::with_capacity(20 + msg_len);
    // Header: Binding Success Response
    buf.extend_from_slice(&0x0101u16.to_be_bytes());
    buf.extend_from_slice(&(msg_len as u16).to_be_bytes());
    buf.extend_from_slice(&MAGIC_COOKIE);
    buf.extend_from_slice(&tx_id.0);

    // XOR-MAPPED-ADDRESS attribute
    buf.extend_from_slice(&0x0020u16.to_be_bytes());
    buf.extend_from_slice(&(attr_value_len as u16).to_be_bytes());
    buf.push(0); // reserved
    buf.push(family);
    buf.extend_from_slice(&xport);
    buf.extend_from_slice(&xor_addr_bytes);

    // Pad to 4-byte boundary
    let pad = padded_value_len - attr_value_len;
    buf.resize(buf.len() + pad, 0);

    let _ = attr_total; // suppress unused warning
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_format() {
        let tx = TxId([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let pkt = request(&tx);

        // Binding request type
        assert_eq!(u16::from_be_bytes([pkt[0], pkt[1]]), 0x0001);
        // Magic cookie at bytes 4..8
        assert_eq!(&pkt[4..8], &MAGIC_COOKIE);
        // Transaction ID at bytes 8..20
        assert_eq!(&pkt[8..20], &tx.0);
        // Total length >= 20 header + some attrs
        assert!(pkt.len() > 20);
        // First two bits zero
        assert_eq!(pkt[0] & 0xC0, 0);
    }

    #[test]
    fn test_parse_response_v4() {
        let tx = TxId::random();
        let addr: std::net::SocketAddr = "1.2.3.4:5678".parse().unwrap();
        let resp = build_response(&tx, addr);
        let (parsed_tx, parsed_addr) = parse_response(&resp).unwrap();
        assert_eq!(parsed_tx, tx);
        assert_eq!(parsed_addr, addr);
    }

    #[test]
    fn test_parse_response_v6() {
        let tx = TxId::random();
        let addr: std::net::SocketAddr = "[2001:db8::1]:4321".parse().unwrap();
        let resp = build_response(&tx, addr);
        let (parsed_tx, parsed_addr) = parse_response(&resp).unwrap();
        assert_eq!(parsed_tx, tx);
        assert_eq!(parsed_addr, addr);
    }

    #[test]
    fn test_is_stun() {
        let tx = TxId::random();
        let pkt = request(&tx);
        assert!(is_stun(&pkt));

        // Disco magic (TS💬) is not STUN
        let disco = {
            let mut d = vec![0x54, 0x53, 0xF0, 0x9F, 0x92, 0xAC];
            d.extend_from_slice(&[0u8; 56]); // pad to HEADER_LEN
            d
        };
        assert!(!is_stun(&disco));

        // WireGuard handshake init (type 1, 148 bytes)
        let mut wg = vec![0u8; 148];
        wg[0] = 1;
        assert!(!is_stun(&wg));
    }

    #[test]
    fn test_round_trip_tx_id() {
        let tx = TxId::random();
        let addr: std::net::SocketAddr = "93.184.216.34:443".parse().unwrap();
        let _req = request(&tx);
        // Simulate server echoing back with same tx_id
        let resp = build_response(&tx, addr);
        let (parsed_tx, parsed_addr) = parse_response(&resp).unwrap();
        assert_eq!(parsed_tx, tx);
        assert_eq!(parsed_addr, addr);
    }
}
