//! Per-peer endpoint state machine for direct connectivity probing.
//!
//! Tracks candidate endpoints, outstanding disco pings, and the current
//! "best" direct address (lowest confirmed latency) for each peer.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::disco;

/// Tracks endpoint state for all peers.
#[doc(hidden)]
pub struct EndpointTracker {
    peers: HashMap<[u8; 32], PeerEndpoint>,
}

struct PeerEndpoint {
    /// Best confirmed direct path: (addr, confirmed_at, latency).
    best_addr: Option<(SocketAddr, Instant, Duration)>,
    /// Candidate endpoints from STUN + netmap + CallMeMaybe.
    candidates: Vec<SocketAddr>,
    /// Outstanding pings: tx_id → (target_addr, sent_at).
    pending_pings: HashMap<[u8; 12], (SocketAddr, Instant)>,
    /// When we last sent a CallMeMaybe to this peer.
    last_call_me_maybe: Option<Instant>,
}

impl EndpointTracker {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    /// Start probing a set of candidate endpoints for a peer.
    /// Returns Ping messages to send.
    pub fn start_probing(
        &mut self,
        peer_key: &[u8; 32],
        candidates: Vec<SocketAddr>,
    ) -> Vec<(SocketAddr, disco::Ping)> {
        let entry = self
            .peers
            .entry(*peer_key)
            .or_insert_with(PeerEndpoint::new);
        entry.candidates = candidates.clone();

        let mut pings = Vec::new();
        for addr in &candidates {
            let mut tx_id = [0u8; 12];
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut tx_id);

            entry.pending_pings.insert(tx_id, (*addr, Instant::now()));

            pings.push((
                *addr,
                disco::Ping {
                    tx_id,
                    node_key: None,
                    padding: 0,
                },
            ));
        }
        pings
    }

    /// Handle an incoming Pong — update the best address if this is better.
    pub fn handle_pong(
        &mut self,
        peer_key: &[u8; 32],
        tx_id: &[u8; 12],
        observed_addr: SocketAddr,
    ) -> Option<SocketAddr> {
        let entry = self.peers.get_mut(peer_key)?;
        let (addr, sent_at) = entry.pending_pings.remove(tx_id)?;
        let latency = sent_at.elapsed();

        tracing::debug!(
            "pong from peer {:02x}{:02x}..{:02x}{:02x} via {addr} in {latency:?} (observed: {observed_addr})",
            peer_key[0], peer_key[1], peer_key[30], peer_key[31],
        );

        let dominated = entry
            .best_addr
            .as_ref()
            .is_none_or(|(_, _, old_lat)| latency < *old_lat);

        if dominated {
            entry.best_addr = Some((addr, Instant::now(), latency));
            Some(addr)
        } else {
            None
        }
    }

    /// Handle a CallMeMaybe — peer is telling us their endpoints to probe.
    pub fn handle_call_me_maybe(
        &mut self,
        peer_key: &[u8; 32],
        endpoints: Vec<SocketAddr>,
    ) -> Vec<(SocketAddr, disco::Ping)> {
        self.start_probing(peer_key, endpoints)
    }

    /// Build a CallMeMaybe from our STUN-discovered endpoints.
    pub fn build_call_me_maybe(
        &mut self,
        peer_key: &[u8; 32],
        our_endpoints: &[SocketAddr],
    ) -> disco::CallMeMaybe {
        if let Some(entry) = self.peers.get_mut(peer_key) {
            entry.last_call_me_maybe = Some(Instant::now());
        } else {
            let mut entry = PeerEndpoint::new();
            entry.last_call_me_maybe = Some(Instant::now());
            self.peers.insert(*peer_key, entry);
        }

        disco::CallMeMaybe {
            endpoints: our_endpoints.to_vec(),
        }
    }

    /// Get the current best direct address for a peer.
    #[allow(dead_code)]
    pub fn best_addr(&self, peer_key: &[u8; 32]) -> Option<SocketAddr> {
        self.peers
            .get(peer_key)
            .and_then(|e| e.best_addr.as_ref())
            .map(|(addr, _, _)| *addr)
    }

    /// Expire stale best addresses (older than 5 minutes) and stale pending
    /// pings (older than 5 seconds).
    #[allow(dead_code)]
    pub fn expire_stale(&mut self) {
        let now = Instant::now();
        for entry in self.peers.values_mut() {
            if let Some((_, confirmed_at, _)) = &entry.best_addr
                && now.duration_since(*confirmed_at) > Duration::from_secs(300) {
                    entry.best_addr = None;
                }
            entry
                .pending_pings
                .retain(|_, (_, sent)| now.duration_since(*sent) < Duration::from_secs(5));
        }
    }
}

impl PeerEndpoint {
    fn new() -> Self {
        Self {
            best_addr: None,
            candidates: Vec::new(),
            pending_pings: HashMap::new(),
            last_call_me_maybe: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_probing() {
        let mut tracker = EndpointTracker::new();
        let peer = [0xAA; 32];
        let candidates = vec![
            "1.2.3.4:41641".parse().unwrap(),
            "5.6.7.8:41641".parse().unwrap(),
        ];

        let pings = tracker.start_probing(&peer, candidates);
        assert_eq!(pings.len(), 2);

        // Each ping should have a unique tx_id
        assert_ne!(pings[0].1.tx_id, pings[1].1.tx_id);
    }

    #[test]
    fn test_handle_pong_updates_best() {
        let mut tracker = EndpointTracker::new();
        let peer = [0xBB; 32];
        let addr: SocketAddr = "1.2.3.4:41641".parse().unwrap();
        let pings = tracker.start_probing(&peer, vec![addr]);

        let tx_id = pings[0].1.tx_id;
        let result =
            tracker.handle_pong(&peer, &tx_id, "5.6.7.8:12345".parse().unwrap());

        assert_eq!(result, Some(addr));
        assert_eq!(tracker.best_addr(&peer), Some(addr));
    }

    #[test]
    fn test_handle_pong_unknown_tx() {
        let mut tracker = EndpointTracker::new();
        let peer = [0xCC; 32];
        let _ = tracker.start_probing(&peer, vec!["1.2.3.4:100".parse().unwrap()]);

        let bogus_tx = [0xFF; 12];
        let result = tracker.handle_pong(&peer, &bogus_tx, "0.0.0.0:0".parse().unwrap());
        assert!(result.is_none());
    }

    #[test]
    fn test_expire_stale_pings() {
        let mut tracker = EndpointTracker::new();
        let peer = [0xDD; 32];
        let _ = tracker.start_probing(&peer, vec!["1.2.3.4:100".parse().unwrap()]);

        // Pings are fresh, so they survive.
        tracker.expire_stale();
        let entry = tracker.peers.get(&peer).unwrap();
        assert_eq!(entry.pending_pings.len(), 1);
    }

    #[test]
    fn test_build_call_me_maybe() {
        let mut tracker = EndpointTracker::new();
        let peer = [0xEE; 32];
        let endpoints = vec![
            "1.2.3.4:41641".parse().unwrap(),
            "[::1]:41641".parse().unwrap(),
        ];

        let cmm = tracker.build_call_me_maybe(&peer, &endpoints);
        assert_eq!(cmm.endpoints.len(), 2);
    }
}
