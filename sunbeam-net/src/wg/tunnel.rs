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
    /// Local boringtun index assigned to this peer's tunnel. Used to route
    /// inbound UDP packets back to the right peer via the receiver_index
    /// field in WireGuard message types 2/3/4.
    local_index: u32,
}

/// Result of encapsulating an outbound IP packet.
///
/// May contain zero, one, or two transport hints. We always prefer to send
/// over every available transport — boringtun's replay protection on the
/// receiver dedupes any duplicates by counter, and going dual-path lets us
/// transparently fall back when one direction is broken (e.g. peer's
/// advertised UDP endpoint is on an unreachable RFC1918 network).
pub(crate) struct EncapAction {
    /// If set, send these bytes via UDP to the given endpoint.
    pub udp: Option<(SocketAddr, Vec<u8>)>,
    /// If set, send these bytes via DERP relay to the given peer key.
    pub derp: Option<([u8; 32], Vec<u8>)>,
}

impl EncapAction {
    pub fn nothing() -> Self {
        Self {
            udp: None,
            derp: None,
        }
    }

    #[allow(dead_code)]
    pub fn is_nothing(&self) -> bool {
        self.udp.is_none() && self.derp.is_none()
    }
}

/// Result of decapsulating an inbound WireGuard packet.
pub(crate) enum DecapAction {
    /// Decrypted IP packet ready for the virtual network stack.
    Packet(Vec<u8>),
    /// Need to send one or more responses (handshake response, cookie, etc.)
    /// Each entry is a separate WG packet that must be sent individually.
    Response(Vec<Vec<u8>>),
    /// Nothing (keep-alive, etc.)
    Nothing,
}

pub(crate) struct TimerAction {
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
                    None,     // no preshared key
                    Some(25), // 25 second persistent keepalive
                    index,
                    None, // no rate limiter
                );

                self.peers.insert(
                    *key_bytes,
                    PeerTunnel {
                        tunn,
                        endpoint: parse_first_endpoint(&node.endpoints),
                        derp_region: parse_derp_region(&node.derp),
                        allowed_ips: parse_allowed_ips(&node.allowed_ips),
                        local_index: index,
                    },
                );
            }
        }
    }

    /// Encapsulate an outbound IP packet for transmission.
    pub fn encapsulate(&mut self, dst_ip: IpAddr, payload: &[u8]) -> EncapAction {
        let peer_key = match self.find_peer_for_ip(dst_ip) {
            Some(k) => *k,
            None => return EncapAction::nothing(),
        };

        let peer = match self.peers.get_mut(&peer_key) {
            Some(p) => p,
            None => return EncapAction::nothing(),
        };

        let mut buf = vec![0u8; BUF_SIZE];
        match peer.tunn.encapsulate(payload, &mut buf) {
            TunnResult::WriteToNetwork(data) => {
                let packet = data.to_vec();
                route_packet(peer, &peer_key, packet)
            }
            TunnResult::Err(e) => {
                tracing::warn!("WG encapsulate error: {e:?} ({} bytes to {dst_ip})", payload.len());
                EncapAction::nothing()
            }
            TunnResult::Done => EncapAction::nothing(),
            // These shouldn't happen during encapsulate, but handle gracefully.
            TunnResult::WriteToTunnelV4(_, _) | TunnResult::WriteToTunnelV6(_, _) => {
                EncapAction::nothing()
            }
        }
    }

    /// Decapsulate an inbound WireGuard packet.
    pub fn decapsulate(&mut self, peer_key: &[u8; 32], packet: &[u8]) -> DecapAction {
        let peer = match self.peers.get_mut(peer_key) {
            Some(p) => p,
            None => {
                tracing::debug!(
                    "decapsulate: no peer for key {:02x}{:02x}..{:02x}{:02x} ({} bytes)",
                    peer_key[0], peer_key[1], peer_key[30], peer_key[31], packet.len()
                );
                return DecapAction::Nothing;
            }
        };

        let mut buf = vec![0u8; BUF_SIZE];
        let result = peer.tunn.decapsulate(None, packet, &mut buf);

        match result {
            TunnResult::WriteToTunnelV4(data, _addr) => DecapAction::Packet(data.to_vec()),
            TunnResult::WriteToTunnelV6(data, _addr) => DecapAction::Packet(data.to_vec()),
            TunnResult::WriteToNetwork(data) => {
                let mut responses = vec![data.to_vec()];
                let mut chain_buf = vec![0u8; BUF_SIZE];
                loop {
                    match peer.tunn.decapsulate(None, &[], &mut chain_buf) {
                        TunnResult::WriteToNetwork(more) => {
                            responses.push(more.to_vec());
                        }
                        TunnResult::Done => break,
                        _ => break,
                    }
                }
                if responses.len() > 1 {
                    tracing::debug!("decap: {} chained response(s)", responses.len() - 1);
                }
                DecapAction::Response(responses)
            }
            TunnResult::Err(e) => {
                tracing::warn!(
                    "WG decapsulate error for peer {:02x}{:02x}..{:02x}{:02x}: {e:?} ({} bytes, type={})",
                    peer_key[0], peer_key[1], peer_key[30], peer_key[31],
                    packet.len(),
                    if packet.is_empty() { 0 } else { packet[0] }
                );
                DecapAction::Nothing
            }
            TunnResult::Done => DecapAction::Nothing,
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
                    actions.push(TimerAction { action });
                }
                TunnResult::Err(e) => {
                    tracing::warn!(
                        "WG tick error for peer {:02x}{:02x}..{:02x}{:02x}: {e:?}",
                        peer_key[0], peer_key[1], peer_key[30], peer_key[31]
                    );
                }
                TunnResult::Done => {}
                TunnResult::WriteToTunnelV4(_, _) | TunnResult::WriteToTunnelV6(_, _) => {}
            }
        }

        actions
    }

    /// Find a peer by the boringtun local index we assigned to it.
    /// WireGuard message types 2/3/4 carry this in the receiver_index field.
    pub fn find_peer_by_local_index(&self, idx: u32) -> Option<[u8; 32]> {
        for (key, peer) in &self.peers {
            if peer.local_index == idx {
                return Some(*key);
            }
        }
        None
    }

    /// Find a peer whose advertised endpoint matches the given socket addr.
    /// Used as a fallback for inbound UDP packets that don't carry our index
    /// (i.e. type-1 handshake initiations from peers we already know about).
    pub fn find_peer_by_endpoint(&self, addr: SocketAddr) -> Option<[u8; 32]> {
        for (key, peer) in &self.peers {
            if peer.endpoint == Some(addr) {
                return Some(*key);
            }
        }
        None
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

    /// Build an `EncapAction` to route a raw WG packet to the given peer,
    /// using whatever transports (UDP endpoint, DERP region) are known.
    pub fn route_to_peer(&self, peer_key: &[u8; 32], data: Vec<u8>) -> EncapAction {
        match self.peers.get(peer_key) {
            Some(peer) => route_packet(peer, peer_key, data),
            None => EncapAction::nothing(),
        }
    }

    /// Update a peer's UDP endpoint from observed traffic (e.g. a received
    /// UDP packet from a new address).
    pub fn update_peer_endpoint(&mut self, peer_key: &[u8; 32], endpoint: SocketAddr) {
        if let Some(peer) = self.peers.get_mut(peer_key)
            && peer.endpoint != Some(endpoint) {
                tracing::debug!(
                    "peer {:02x}{:02x}..{:02x}{:02x} endpoint changed: {:?} → {endpoint}",
                    peer_key[0], peer_key[1], peer_key[30], peer_key[31],
                    peer.endpoint,
                );
                peer.endpoint = Some(endpoint);
            }
    }

    /// Return all peer public keys.
    pub fn peer_keys(&self) -> Vec<[u8; 32]> {
        self.peers.keys().copied().collect()
    }

    /// Expose peer count for testing.
    #[cfg(test)]
    fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

/// Decide how to route a WireGuard packet for a given peer. Returns both
/// transports when both are available — the receiver dedupes via WG counter.
fn route_packet(peer: &PeerTunnel, peer_key: &[u8; 32], data: Vec<u8>) -> EncapAction {
    let mut action = EncapAction::nothing();
    if peer.derp_region.is_some() {
        action.derp = Some((*peer_key, data.clone()));
    }
    if let Some(endpoint) = peer.endpoint {
        action.udp = Some((endpoint, data));
    }
    action
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
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::types::{HostInfo, Node};
    use std::net::Ipv4Addr;

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
            net_info: None,
        }
    }

    fn make_peer_node(key_bytes: &[u8; 32], allowed_ips: Vec<&str>) -> Node {
        Node {
            id: 1,
            key: format!("nodekey:{}", hex_encode(key_bytes)),
            disco_key: "discokey:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
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

    #[test]
    fn test_update_peers_idempotent() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _) = generate_key();
        let node = make_peer_node(&peer_pub, vec!["100.64.0.2/32"]);

        tunnel.update_peers(std::slice::from_ref(&node));
        assert_eq!(tunnel.peer_count(), 1);
        let idx_after_first = tunnel.next_index;

        // Second call with same peer should not increment next_index.
        tunnel.update_peers(&[node]);
        assert_eq!(tunnel.peer_count(), 1);
        assert_eq!(tunnel.next_index, idx_after_first);
    }

    #[test]
    fn test_update_peers_endpoint_change() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _) = generate_key();
        let mut node = make_peer_node(&peer_pub, vec!["100.64.0.2/32"]);
        tunnel.update_peers(&[node.clone()]);
        assert_eq!(
            tunnel.peers.get(&peer_pub).unwrap().endpoint,
            Some("1.2.3.4:41641".parse().unwrap())
        );

        // Change endpoint.
        node.endpoints = vec!["5.6.7.8:41641".into()];
        tunnel.update_peers(&[node]);
        assert_eq!(
            tunnel.peers.get(&peer_pub).unwrap().endpoint,
            Some("5.6.7.8:41641".parse().unwrap())
        );
    }

    #[test]
    fn test_update_peers_key_rotation() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (old_pub, _) = generate_key();
        let (new_pub, _) = generate_key();

        let old_node = make_peer_node(&old_pub, vec!["100.64.0.2/32"]);
        tunnel.update_peers(&[old_node]);
        assert!(tunnel.peers.contains_key(&old_pub));

        // Replace with new key — old peer should be removed.
        let new_node = make_peer_node(&new_pub, vec!["100.64.0.2/32"]);
        tunnel.update_peers(&[new_node]);
        assert!(!tunnel.peers.contains_key(&old_pub));
        assert!(tunnel.peers.contains_key(&new_pub));
        assert_eq!(tunnel.peer_count(), 1);
    }

    #[test]
    fn test_remove_peer_drops_all_session_state() {
        // Two peers, both with non-trivial state (endpoint, allowed_ips, and
        // boringtun handshake-init state from a forced encapsulate call).
        // Removing one must drop *all* per-peer state; the other is untouched.
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (gone_pub, _) = generate_key();
        let (kept_pub, _) = generate_key();
        let gone_node = {
            let mut n = make_peer_node(&gone_pub, vec!["100.64.0.2/32"]);
            n.endpoints = vec!["1.2.3.4:41641".into()];
            n
        };
        let kept_node = {
            let mut n = make_peer_node(&kept_pub, vec!["100.64.0.3/32"]);
            n.endpoints = vec!["5.6.7.8:41641".into()];
            n
        };
        tunnel.update_peers(&[gone_node, kept_node.clone()]);

        // Force boringtun to build a session (handshake initiation) for the
        // peer we're about to remove, so per-peer state is non-trivial.
        let gone_idx = tunnel.peers.get(&gone_pub).unwrap().local_index;
        let gone_ep: SocketAddr = "1.2.3.4:41641".parse().unwrap();
        let action = tunnel.encapsulate(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2)), b"ping");
        assert!(action.udp.is_some(), "expected handshake init to emit UDP");

        // Drop the peer via the real removal path: a netmap update that omits it.
        tunnel.update_peers(&[kept_node]);

        // Peer map lookup: gone.
        assert!(!tunnel.peers.contains_key(&gone_pub));
        assert_eq!(tunnel.peer_count(), 1);
        // Index, endpoint, and IP lookups all fail for the removed peer.
        assert_eq!(tunnel.find_peer_by_local_index(gone_idx), None);
        assert_eq!(tunnel.find_peer_by_endpoint(gone_ep), None);
        assert_eq!(
            tunnel.find_peer_for_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2))),
            None
        );
        // Inbound packets claiming to be from the removed peer are dropped.
        assert!(matches!(
            tunnel.decapsulate(&gone_pub, &[4u8; 32]),
            DecapAction::Nothing
        ));
        // Routing to the removed peer yields nothing on either transport.
        assert!(tunnel.route_to_peer(&gone_pub, vec![1, 2, 3]).is_nothing());
        // The surviving peer is untouched.
        assert!(tunnel.peers.contains_key(&kept_pub));
        assert_eq!(
            tunnel.find_peer_for_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 3))),
            Some(&kept_pub)
        );
    }

    #[test]
    fn test_route_to_peer() {
        let (_, my_secret) = generate_key();
        let mut tunnel = WgTunnel::new(my_secret);

        let (peer_pub, _) = generate_key();
        let node = make_peer_node(&peer_pub, vec!["100.64.0.2/32"]);
        tunnel.update_peers(&[node]);

        let action = tunnel.route_to_peer(&peer_pub, vec![1, 2, 3]);
        assert!(action.udp.is_some());
        assert!(action.derp.is_some());

        // Unknown peer returns nothing.
        let (unknown, _) = generate_key();
        let action = tunnel.route_to_peer(&unknown, vec![1, 2, 3]);
        assert!(action.is_nothing());
    }
}
