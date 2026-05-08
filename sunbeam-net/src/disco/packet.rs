//! Packet classifier for the shared UDP socket: distinguish STUN, disco, and
//! WireGuard traffic.

/// The kind of packet received on the multiplexed UDP socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    /// Stun.
    Stun,
    /// Disco.
    Disco,
    /// Wireguard.
    WireGuard,
    /// Unknown.
    Unknown,
}

/// Classify a raw UDP payload as STUN, disco, WireGuard, or unknown.
pub fn classify(buf: &[u8]) -> PacketKind {
    if crate::stun::is_stun(buf) {
        return PacketKind::Stun;
    }
    if buf.len() >= super::HEADER_LEN && buf[..6] == super::MAGIC {
        return PacketKind::Disco;
    }
    // WireGuard message types 1-4 have a 4-byte type field at the start.
    // Type 1 (HandshakeInit): 148 bytes, type 2 (HandshakeResp): 92 bytes,
    // type 3 (CookieReply): 64 bytes, type 4 (Transport): variable.
    if buf.len() >= 4 && matches!(buf[0], 1..=4) {
        return PacketKind::WireGuard;
    }
    PacketKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stun;

    #[test]
    fn test_classify_stun() {
        let tx = stun::TxId::random();
        let addr: std::net::SocketAddr = "1.2.3.4:5678".parse().unwrap();
        // Build a STUN success response
        let resp = stun::build_response(&tx, addr);
        assert_eq!(classify(&resp), PacketKind::Stun);
    }

    #[test]
    fn test_classify_disco() {
        let mut pkt = Vec::with_capacity(super::super::HEADER_LEN + 16);
        pkt.extend_from_slice(&super::super::MAGIC);
        pkt.extend_from_slice(&[0u8; 32]); // fake pubkey
        pkt.extend_from_slice(&[0u8; 24]); // fake nonce
        pkt.extend_from_slice(&[0u8; 16]); // some ciphertext
        assert_eq!(classify(&pkt), PacketKind::Disco);
    }

    #[test]
    fn test_classify_wireguard() {
        // Type 1 (HandshakeInit) — 148 bytes
        let mut wg_init = vec![0u8; 148];
        wg_init[0] = 1;
        assert_eq!(classify(&wg_init), PacketKind::WireGuard);

        // Type 4 (Transport) — 100 bytes
        let mut wg_data = vec![0u8; 100];
        wg_data[0] = 4;
        assert_eq!(classify(&wg_data), PacketKind::WireGuard);
    }

    #[test]
    fn test_classify_unknown() {
        let garbage = vec![0xFF, 0xFE, 0xFD, 0xFC, 0xFB, 0xFA, 0xF9, 0xF8];
        assert_eq!(classify(&garbage), PacketKind::Unknown);
    }

    #[test]
    fn test_classify_empty() {
        assert_eq!(classify(&[]), PacketKind::Unknown);
    }
}
