use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use boringtun::noise::{Tunn, TunnResult};
use ipnet::IpNet;
use x25519_dalek::{PublicKey, StaticSecret};

/// Manages WireGuard tunnels for all peers.
pub(crate) struct WgTunnel {
    private_key: StaticSecret,
    peers: HashMap<[u8; 32], PeerTunnel>,
    /// Index counter for boringtun tunnel creation.
    next_index: u32,
}

struct PeerTunnel {
    tunn: Tunn,
    endpoint: Option<SocketAddr>,
    derp_region: Option<u16>,
    allowed_ips: Vec<IpNet>,
}

/// Result of encapsulating an outbound IP packet.
pub(crate) enum EncapAction {
    /// Send these bytes over UDP to the given endpoint.
    SendUdp { endpoint: SocketAddr, data: Vec<u8> },
    /// Send these bytes via DERP relay.
    SendDerp { dest_key: [u8; 32], data: Vec<u8> },
    /// Nothing to send (handshake pending, etc.)
    Nothing,
}

/// Result of decapsulating an inbound WireGuard packet.
pub(crate) enum DecapAction {
    /// Decrypted IP packet ready for the virtual network stack.
    Packet(Vec<u8>),
    /// Need to send a response (handshake response, cookie, etc.)
    Response(Vec<u8>),
    /// Nothing (keep-alive, etc.)
    Nothing,
}

pub(crate) struct TimerAction {
    pub peer_key: [u8; 32],
    pub action: EncapAction,
}

/// Size of the scratch buffer for boringtun operations.
const BUF_SIZE: usize = 65536;

impl WgTunnel {
    pub fn new(private_key: StaticSecret) -> Self {
        Self {
            private_key,
            peers: HashMap::new(),
            next_index: 0,
        }
    }

    /// Update the peer table from a network map.
    /// Adds new peers, removes stale ones, updates endpoints.
    pub fn update_peers(&mut self, peers: &[crate::proto::types::Node]) {
        // Collect the set of peer keys we should have.
        let mut desired_keys: HashMap<[u8; 32], &crate::proto::types::Node> = HashMap::new();
        for node in peers {
            if let Some(key) = parse_node_key(&node.key) {
                desired_keys.insert(key, node);
            }
        }

        // Remove peers not in the desired set.
        self.peers.retain(|k, _| desired_keys.contains_key(k));

        // Add or update peers.
        for (key_bytes, node) in &desired_keys {
            if self.peers.contains_key(key_bytes) {
                // Update endpoint and DERP region on existing peer.
                let peer = self.peers.get_mut(key_bytes).unwrap();
                peer.endpoint = parse_first_endpoint(&node.endpoints);
                peer.derp_region = parse_derp_region(&node.derp);
                peer.allowed_ips = parse_allowed_ips(&node.allowed_ips);
            } else {
                // Create a new boringtun tunnel for this peer.
                let peer_public = PublicKey::from(*key_bytes);
                let index = self.next_index;
                self.next_index = self.next_index.wrapping_add(1);

                let tunn = Tunn::new(
                    self.private_key.clone(),
                    peer_public,
                    None,  // no preshared key
                    Some(25),  // 25 second persistent keepalive
                    index,
                    None,  // no rate limiter
                );

                self.peers.insert(*key_bytes, PeerTunnel {
                    tunn,
                    endpoint: parse_first_endpoint(&node.endpoints),
                    derp_region: parse_derp_region(&node.derp),
                    allowed_ips: parse_allowed_ips(&node.allowed_ips),
                });
            }
        }
    }

    /// Encapsulate an outbound IP packet for transmission.
    pub fn encapsulate(&mut self, dst_ip: IpAddr, payload: &[u8]) -> EncapAction {
        let peer_key = match self.find_peer_for_ip(dst_ip) {
            Some(k) => *k,
            None => return EncapAction::Nothing,
        };

        let peer = match self.peers.get_mut(&peer_key) {
            Some(p) => p,
            None => return EncapAction::Nothing,
        };

        let mut buf = vec![0u8; BUF_SIZE];
        match peer.tunn.encapsulate(payload, &mut buf) {
            TunnResult::WriteToNetwork(data) => {
                let packet = data.to_vec();
                route_packet(peer, &peer_key, packet)
            }
            TunnResult::Err(_) | TunnResult::Done => EncapAction::Nothing,
            // These shouldn't happen during encapsulate, but handle gracefully.
            TunnResult::WriteToTunnelV4(_, _) | TunnResult::WriteToTunnelV6(_, _) => {
                EncapAction::Nothing
            }
        }
    }

    /// Decapsulate an inbound WireGuard packet.
    pub fn decapsulate(&mut self, peer_key: &[u8; 32], packet: &[u8]) -> DecapAction {
        let peer = match self.peers.get_mut(peer_key) {
            Some(p) => p,
            None => return DecapAction::Nothing,
        };

        let mut buf = vec![0u8; BUF_SIZE];
        let result = peer.tunn.decapsulate(None, packet, &mut buf);

        match result {
            TunnResult::WriteToTunnelV4(data, _addr) => DecapAction::Packet(data.to_vec()),
            TunnResult::WriteToTunnelV6(data, _addr) => DecapAction::Packet(data.to_vec()),
            TunnResult::WriteToNetwork(data) => {
                // This is a handshake response or cookie that needs to be sent back.
                let response = data.to_vec();
                // Check for chained results — loop to drain.
                let mut chain_buf = vec![0u8; BUF_SIZE];
                loop {
                    match peer.tunn.decapsulate(None, &[], &mut chain_buf) {
                        TunnResult::WriteToNetwork(more) => {
                            // Multiple chained responses; we return the first.
                            let _ = more;
                        }
                        TunnResult::Done => break,
                        _ => break,
                    }
                }
                DecapAction::Response(response)
            }
            TunnResult::Err(_) | TunnResult::Done => DecapAction::Nothing,
        }
    }

    /// Tick all tunnels for timer-based actions (keepalives, handshake retries).
    pub fn tick(&mut self) -> Vec<TimerAction> {
        let mut actions = Vec::new();
        let peer_keys: Vec<[u8; 32]> = self.peers.keys().copied().collect();

        for peer_key in peer_keys {
            let peer = match self.peers.get_mut(&peer_key) {
                Some(p) => p,
                None => continue,
            };

            let mut buf = vec![0u8; BUF_SIZE];
            let result = peer.tunn.update_timers(&mut buf);

            match result {
                TunnResult::WriteToNetwork(data) => {
                    let packet = data.to_vec();
                    let action = route_packet(peer, &peer_key, packet);
                    actions.push(TimerAction {
                        peer_key,
                        action,
                    });
                }
                TunnResult::Err(_) | TunnResult::Done => {}
                TunnResult::WriteToTunnelV4(_, _) | TunnResult::WriteToTunnelV6(_, _) => {}
            }
        }

        actions
    }

    /// Find which peer owns a given IP address.
    fn find_peer_for_ip(&self, ip: IpAddr) -> Option<&[u8; 32]> {
        for (key, peer) in &self.peers {
            for net in &peer.allowed_ips {
                if net.contains(&ip) {
                    return Some(key);
                }
            }
        }
        None
    }

    /// Expose peer count for testing.
    #[cfg(test)]
    fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

/// Decide how to route a WireGuard packet for a given peer.
fn route_packet(peer: &PeerTunnel, peer_key: &[u8; 32], data: Vec<u8>) -> EncapAction {
    if let Some(endpoint) = peer.endpoint {
        EncapAction::SendUdp { endpoint, data }
    } else if peer.derp_region.is_some() {
        EncapAction::SendDerp {
            dest_key: *peer_key,
            data,
        }
    } else {
        EncapAction::Nothing
    }
}

/// Parse a "nodekey:<hex>" string into raw 32-byte key.
fn parse_node_key(s: &str) -> Option<[u8; 32]> {
    let hex_str = s.strip_prefix("nodekey:")?;
    let bytes = hex_decode(hex_str)?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(arr)
}

/// Parse the first endpoint from a list of "ip:port" strings.
fn parse_first_endpoint(endpoints: &[String]) -> Option<SocketAddr> {
    endpoints.first().and_then(|s| s.parse().ok())
}

/// Parse DERP region from a DERP string like "127.3.3.40:<region_id>".
fn parse_derp_region(derp: &str) -> Option<u16> {
    // Tailscale/Headscale DERP format: "127.3.3.40:<region_id>"
    let parts: Vec<&str> = derp.split(':').collect();
    if parts.len() == 2 {
        parts[1].parse().ok()
    } else {
        None
    }
}

/// Parse allowed IP strings into IpNet values.
fn parse_allowed_ips(ips: &[String]) -> Vec<IpNet> {
    ips.iter().filter_map(|s| s.parse().ok()).collect()
}

/// Simple hex decoder.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Simple hex encoder.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use super::*;
    use crate::proto::types::{HostInfo, Node};

    fn test_hostinfo() -> HostInfo {
        HostInfo {
            go_arch: "arm64".into(),
            go_os: "linux".into(),
            go_version: "sunbeam-net/0.1".into(),
            hostname: "test".into(),
            os: "linux".into(),
            os_version: "6.1".into(),
            device_model: None,
            frontend_log_id: None,
            backend_log_id: None,
        }
    }

    fn make_peer_node(key_bytes: &[u8; 32], allowed_ips: Vec<&str>) -> Node {
        Node {
            id: 1,
            key: format!("nodekey:{}", hex_encode(key_bytes)),
            disco_key: "discokey:0000000000000000000000000000000000000000000000000000000000000000".into(),
            addresses: vec!["100.64.0.2/32".into()],
            allowed_ips: allowed_ips.into_iter().map(String::from).collect(),
            endpoints: vec!["1.2.3.4:41641".into()],
            derp: "127.3.3.40:1".into(),
            hostinfo: test_hostinfo(),
            name: "peer.example.com".into(),
            online: Some(true),
            machine_authorized: true,
        }
    }

    fn generate_key() -> ([u8; 32], StaticSecret) {
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&secret);
        (*public.as_bytes(), secret)
    }

    #[test]
    fn test_new_tunnel() {
        let (_, secret) = generate_key();
        let tunnel = WgTunnel::new(secret);
        assert_eq!(tunnel.peer_count(), 0);
    }

    #[test]
    fn test_update_peers_adds_peer() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _peer_secret) = generate_key();
        let node = make_peer_node(&peer_pub, vec!["100.64.0.2/32"]);

        tunnel.update_peers(&[node]);
        assert_eq!(tunnel.peer_count(), 1);
        assert!(tunnel.peers.contains_key(&peer_pub));
    }

    #[test]
    fn test_update_peers_removes_stale() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _) = generate_key();
        let node = make_peer_node(&peer_pub, vec!["100.64.0.2/32"]);
        tunnel.update_peers(&[node]);
        assert_eq!(tunnel.peer_count(), 1);

        // Update with empty list — stale peer should be removed.
        tunnel.update_peers(&[]);
        assert_eq!(tunnel.peer_count(), 0);
    }

    #[test]
    fn test_find_peer_for_ip() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _) = generate_key();
        let node = make_peer_node(&peer_pub, vec!["100.64.0.0/24"]);
        tunnel.update_peers(&[node]);

        let found = tunnel.find_peer_for_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 5)));
        assert_eq!(found, Some(&peer_pub));
    }

    #[test]
    fn test_find_peer_no_match() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _) = generate_key();
        let node = make_peer_node(&peer_pub, vec!["100.64.0.0/24"]);
        tunnel.update_peers(&[node]);

        // IP outside the allowed range.
        let found = tunnel.find_peer_for_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(found.is_none());
    }
}
