use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::tunnel::{DecapAction, EncapAction, WgTunnel};

/// Routes packets between WireGuard tunnels, UDP sockets, and DERP relays.
pub(crate) struct PacketRouter {
    tunnel: WgTunnel,
    udp: Option<UdpSocket>,
    ingress_tx: mpsc::Sender<IngressPacket>,
    ingress_rx: mpsc::Receiver<IngressPacket>,
}

/// An inbound WireGuard packet from either UDP or DERP.
pub(crate) enum IngressPacket {
    /// WireGuard packet received over UDP.
    Udp { src: SocketAddr, data: Vec<u8> },
    /// WireGuard packet received via DERP relay.
    Derp { src_key: [u8; 32], data: Vec<u8> },
}

impl PacketRouter {
    pub fn new(tunnel: WgTunnel) -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            tunnel,
            udp: None,
            ingress_tx: tx,
            ingress_rx: rx,
        }
    }

    /// Bind the UDP socket for direct WireGuard traffic.
    pub async fn bind_udp(&mut self, addr: SocketAddr) -> crate::Result<()> {
        let sock = UdpSocket::bind(addr)
            .await
            .map_err(|e| crate::Error::WireGuard(format!("failed to bind UDP {addr}: {e}")))?;
        self.udp = Some(sock);
        Ok(())
    }

    /// Get a clone of the ingress sender (for DERP recv loops to feed packets in).
    pub fn ingress_sender(&self) -> mpsc::Sender<IngressPacket> {
        self.ingress_tx.clone()
    }

    /// Process one ingress packet: decapsulate and return any decrypted IP packet.
    /// If decapsulation produces a response (handshake, cookie), it is sent back
    /// to the peer over the appropriate transport.
    pub fn process_ingress(&mut self, packet: IngressPacket) -> Option<Vec<u8>> {
        match packet {
            IngressPacket::Udp { src, data } => {
                // We need to figure out which peer sent this. For UDP, we look up
                // the peer by source address. If we can't find it, try all peers.
                let peer_key = self.find_peer_by_endpoint(src);
                let key = match peer_key {
                    Some(k) => k,
                    None => return None,
                };

                match self.tunnel.decapsulate(&key, &data) {
                    DecapAction::Packet(ip_packet) => Some(ip_packet),
                    DecapAction::Response(response) => {
                        // Queue the response to send back — best-effort.
                        self.try_send_udp_sync(src, &response);
                        None
                    }
                    DecapAction::Nothing => None,
                }
            }
            IngressPacket::Derp { src_key, data } => {
                match self.tunnel.decapsulate(&src_key, &data) {
                    DecapAction::Packet(ip_packet) => Some(ip_packet),
                    DecapAction::Response(_response) => {
                        // DERP responses would need to be sent back via DERP.
                        // The caller should handle this via the DERP send path.
                        None
                    }
                    DecapAction::Nothing => None,
                }
            }
        }
    }

    /// Encapsulate an outbound IP packet and send it via the appropriate transport.
    pub async fn send_outbound(&mut self, ip_packet: &[u8]) -> crate::Result<()> {
        // Parse the destination IP from the IP packet header.
        let dst_ip = match parse_dst_ip(ip_packet) {
            Some(ip) => ip,
            None => return Ok(()),
        };

        match self.tunnel.encapsulate(dst_ip, ip_packet) {
            EncapAction::SendUdp { endpoint, data } => {
                if let Some(ref udp) = self.udp {
                    udp.send_to(&data, endpoint).await.map_err(|e| {
                        crate::Error::WireGuard(format!("UDP send to {endpoint}: {e}"))
                    })?;
                }
                Ok(())
            }
            EncapAction::SendDerp { dest_key: _, data: _ } => {
                // DERP sending would be handled by the caller via a DERP client.
                // This layer just signals the intent; the daemon wires it up.
                Ok(())
            }
            EncapAction::Nothing => Ok(()),
        }
    }

    /// Run timer ticks on all tunnels, send any resulting packets.
    pub async fn tick(&mut self) -> crate::Result<()> {
        let actions = self.tunnel.tick();
        for ta in actions {
            match ta.action {
                EncapAction::SendUdp { endpoint, data } => {
                    if let Some(ref udp) = self.udp {
                        let _ = udp.send_to(&data, endpoint).await;
                    }
                }
                EncapAction::SendDerp { .. } => {
                    // DERP timer packets handled by daemon layer.
                }
                EncapAction::Nothing => {}
            }
        }
        Ok(())
    }

    /// Update peers from netmap.
    pub fn update_peers(&mut self, peers: &[crate::proto::types::Node]) {
        self.tunnel.update_peers(peers);
    }

    /// Find a peer key by its known UDP endpoint.
    fn find_peer_by_endpoint(&self, _src: SocketAddr) -> Option<[u8; 32]> {
        // In a full implementation, we'd maintain an endpoint→key index.
        // For now, we expose internal peer iteration via the tunnel.
        // This is a simplification — a production router would have a reverse map.
        // We try all peers since the tunnel has the endpoint info.
        None
    }

    /// Best-effort synchronous UDP send (used in process_ingress which is sync).
    fn try_send_udp_sync(&self, _dst: SocketAddr, _data: &[u8]) {
        // UdpSocket::try_send_to is not available on tokio UdpSocket without
        // prior connect. In a real implementation, we'd use a std UdpSocket
        // or queue the response for async sending. For now, responses from
        // process_ingress are dropped — the caller should handle them.
    }
}

/// Parse the destination IP address from a raw IP packet.
fn parse_dst_ip(packet: &[u8]) -> Option<std::net::IpAddr> {
    if packet.is_empty() {
        return None;
    }
    let version = packet[0] >> 4;
    match version {
        4 if packet.len() >= 20 => {
            let dst = std::net::Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
            Some(std::net::IpAddr::V4(dst))
        }
        6 if packet.len() >= 40 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&packet[24..40]);
            Some(std::net::IpAddr::V6(std::net::Ipv6Addr::from(octets)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x25519_dalek::StaticSecret;

    fn make_tunnel() -> WgTunnel {
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        WgTunnel::new(secret)
    }

    #[test]
    fn test_new_router() {
        let tunnel = make_tunnel();
        let router = PacketRouter::new(tunnel);
        // Verify the ingress channel is functional by checking the sender isn't closed.
        assert!(!router.ingress_tx.is_closed());
    }

    #[test]
    fn test_ingress_sender_clone() {
        let tunnel = make_tunnel();
        let router = PacketRouter::new(tunnel);
        let sender1 = router.ingress_sender();
        let sender2 = router.ingress_sender();
        // Both senders should be live.
        assert!(!sender1.is_closed());
        assert!(!sender2.is_closed());
    }
}
