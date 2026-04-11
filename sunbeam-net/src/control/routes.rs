//! Route table mapping advertised peer prefixes → owning peer node key.
//!
//! Phase 1 of the subnet-routing feature: consume each peer's `allowed_ips`
//! from the netmap, filter them through a CIDR whitelist, and expose a
//! longest-prefix-match `resolve` lookup. Phase 2 (SOCKS5) will consume this
//! table to pick which peer to tunnel a given destination IP through.
//!
//! The table itself does NOT change routing decisions in this phase — the
//! TCP proxy still targets a single hardcoded destination. We build and
//! maintain the table so it is ready for Phase 2 without requiring another
//! round of lifecycle wiring.

use std::net::IpAddr;

use ipnet::IpNet;

use crate::proto::types::Node;

/// A longest-prefix-match table mapping advertised CIDRs to owning peer
/// node keys.
///
/// Storage is a simple `Vec<(IpNet, String)>` kept sorted by prefix length
/// descending. A linear scan picks the first entry whose prefix contains
/// the query — that's the longest match by construction. For ~30 cluster
/// CIDRs this is fine; swap for a trie only if profiling says so.
#[derive(Debug, Default, Clone)]
pub struct RouteTable {
    /// Sorted by descending prefix length so the first hit in `resolve`
    /// is the longest match. Ties broken by insertion order.
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    net: IpNet,
    node_key: String,
}

impl RouteTable {
    /// Create an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the node key of the peer whose `allowed_ips` contains `ip`
    /// with the longest matching prefix. Returns `None` if nothing matches.
    ///
    /// O(n) scan over sorted-by-prefix-length entries.
    pub fn resolve(&self, ip: IpAddr) -> Option<&str> {
        for entry in &self.entries {
            if entry.net.contains(&ip) {
                return Some(&entry.node_key);
            }
        }
        None
    }

    /// Replace the entire table with routes built from `peers`, filtered
    /// by `whitelist`. Any advertised prefix that is not strictly a subnet
    /// of some whitelist entry is rejected.
    pub fn rebuild(&mut self, peers: &[Node], whitelist: &[IpNet]) {
        self.entries.clear();
        for peer in peers {
            self.insert_peer_routes(peer, whitelist);
        }
        self.sort_by_prefix_desc();
    }

    /// Upsert routes for the given peers. Existing routes owned by each
    /// peer are removed first, then their current `allowed_ips` are
    /// inserted. Other peers' routes are untouched.
    pub fn apply_changed(&mut self, peers: &[Node], whitelist: &[IpNet]) {
        for peer in peers {
            self.entries.retain(|e| e.node_key != peer.key);
            self.insert_peer_routes(peer, whitelist);
        }
        self.sort_by_prefix_desc();
    }

    /// Remove all routes owned by any of `node_keys`.
    pub fn apply_removed(&mut self, node_keys: &[String]) {
        if node_keys.is_empty() {
            return;
        }
        self.entries
            .retain(|e| !node_keys.iter().any(|k| k == &e.node_key));
    }

    /// Iterate all `(prefix, node_key)` entries in the table.
    ///
    /// Order is "longest prefix first" — callers that care should not
    /// rely on it, but it is stable for diagnostics.
    pub fn routes(&self) -> impl Iterator<Item = (IpNet, &str)> {
        self.entries.iter().map(|e| (e.net, e.node_key.as_str()))
    }

    /// Number of entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert_peer_routes(&mut self, peer: &Node, whitelist: &[IpNet]) {
        for raw in &peer.allowed_ips {
            match raw.parse::<IpNet>() {
                Ok(net) => {
                    if !is_whitelisted(&net, whitelist) {
                        tracing::debug!(
                            peer = %peer.key,
                            cidr = %net,
                            "route rejected: not a subnet of any whitelist entry"
                        );
                        continue;
                    }
                    self.entries.push(Entry {
                        net,
                        node_key: peer.key.clone(),
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        peer = %peer.key,
                        cidr = %raw,
                        "skipping malformed CIDR in allowed_ips: {e}"
                    );
                }
            }
        }
    }

    fn sort_by_prefix_desc(&mut self) {
        // Sort by descending prefix length. `sort_by_key` is stable, so
        // insertion order is preserved among ties. We negate the key via
        // `u8::MAX - k` to get descending order without `Reverse`.
        self.entries.sort_by_key(|e| u8::MAX - e.net.prefix_len());
    }
}

/// Strict containment check: `net` must be a subnet (or equal) of at least
/// one entry in `whitelist`. We deliberately do NOT clip — we want a route
/// to be fully inside a permitted range, otherwise a malicious peer could
/// advertise `0.0.0.0/0` and hijack all traffic after partial clipping.
fn is_whitelisted(net: &IpNet, whitelist: &[IpNet]) -> bool {
    whitelist.iter().any(|w| is_subnet_of(net, w))
}

/// True if `a` is fully contained in `b` (including equality).
///
/// `ipnet::IpNet::contains` on another `IpNet` does exactly this (it
/// requires `a`'s prefix length to be ≥ `b`'s and `a`'s network to be
/// inside `b`), but we name it explicitly so the intent stays obvious
/// at the call site.
fn is_subnet_of(a: &IpNet, b: &IpNet) -> bool {
    b.contains(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::types::{HostInfo, Node};

    fn node(key: &str, cidrs: &[&str]) -> Node {
        Node {
            id: 0,
            key: key.to_string(),
            disco_key: format!("discokey:{key}"),
            addresses: vec![],
            allowed_ips: cidrs.iter().map(|s| (*s).to_string()).collect(),
            endpoints: vec![],
            derp: "127.3.3.40:1".to_string(),
            hostinfo: HostInfo::default(),
            name: format!("{key}.test"),
            online: Some(true),
            machine_authorized: true,
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }

    fn default_whitelist() -> Vec<IpNet> {
        vec![
            net("10.0.0.0/8"),
            net("172.16.0.0/12"),
            net("192.168.0.0/16"),
            net("100.64.0.0/10"),
            net("fd7a:115c:a1e0::/48"),
        ]
    }

    #[test]
    fn empty_table_resolves_to_none() {
        let table = RouteTable::new();
        assert!(table.resolve(ip("10.0.0.1")).is_none());
        assert!(table.resolve(ip("::1")).is_none());
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn single_peer_single_cidr() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[node("nodekey:aa", &["10.42.0.0/16"])],
            &default_whitelist(),
        );
        assert_eq!(table.resolve(ip("10.42.0.5")), Some("nodekey:aa"));
        assert_eq!(table.resolve(ip("10.42.255.255")), Some("nodekey:aa"));
        // Outside the advertised prefix, inside whitelist — still None
        // because nobody else advertised it.
        assert_eq!(table.resolve(ip("10.43.0.1")), None);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn longest_prefix_wins_between_peers() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[
                node("nodekey:broad", &["10.0.0.0/8"]),
                node("nodekey:narrow", &["10.42.0.0/16"]),
            ],
            &default_whitelist(),
        );
        assert_eq!(table.resolve(ip("10.42.0.5")), Some("nodekey:narrow"));
        assert_eq!(table.resolve(ip("10.1.0.5")), Some("nodekey:broad"));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn reject_default_route_without_matching_whitelist() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[node("nodekey:evil", &["0.0.0.0/0"])],
            &default_whitelist(),
        );
        assert_eq!(table.resolve(ip("8.8.8.8")), None);
        assert_eq!(table.resolve(ip("10.0.0.1")), None);
        assert!(table.is_empty());
    }

    #[test]
    fn reject_route_partly_outside_whitelist() {
        // Peer advertises /16 but whitelist only covers /17: the /16
        // escapes the whitelist so it's rejected outright (no clipping).
        let mut table = RouteTable::new();
        table.rebuild(
            &[node("nodekey:p", &["10.42.0.0/16"])],
            &[net("10.42.0.0/17")],
        );
        assert_eq!(table.resolve(ip("10.42.0.5")), None);
        assert!(table.is_empty());
    }

    #[test]
    fn accept_route_fully_inside_whitelist() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[node("nodekey:p", &["10.42.128.0/17"])],
            &[net("10.42.0.0/16")],
        );
        assert_eq!(table.resolve(ip("10.42.200.1")), Some("nodekey:p"));
        assert_eq!(table.resolve(ip("10.42.0.1")), None);
    }

    #[test]
    fn equal_prefix_is_accepted() {
        let mut table = RouteTable::new();
        table.rebuild(&[node("nodekey:p", &["10.0.0.0/8"])], &[net("10.0.0.0/8")]);
        assert_eq!(table.resolve(ip("10.5.5.5")), Some("nodekey:p"));
    }

    #[test]
    fn apply_removed_drops_only_the_named_peer() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[
                node("nodekey:a", &["10.1.0.0/16"]),
                node("nodekey:b", &["10.2.0.0/16"]),
                node("nodekey:c", &["10.3.0.0/16"]),
            ],
            &default_whitelist(),
        );
        assert_eq!(table.len(), 3);

        table.apply_removed(&["nodekey:b".to_string()]);
        assert_eq!(table.len(), 2);
        assert_eq!(table.resolve(ip("10.1.0.1")), Some("nodekey:a"));
        assert_eq!(table.resolve(ip("10.2.0.1")), None);
        assert_eq!(table.resolve(ip("10.3.0.1")), Some("nodekey:c"));
    }

    #[test]
    fn apply_removed_empty_is_noop() {
        let mut table = RouteTable::new();
        table.rebuild(&[node("nodekey:a", &["10.1.0.0/16"])], &default_whitelist());
        table.apply_removed(&[]);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn apply_changed_upserts_without_duplicating() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[
                node("nodekey:a", &["10.1.0.0/16"]),
                node("nodekey:b", &["10.2.0.0/16"]),
            ],
            &default_whitelist(),
        );

        // Replace `nodekey:a`'s advertisements with a wider set.
        table.apply_changed(
            &[node("nodekey:a", &["10.1.0.0/16", "10.4.0.0/16"])],
            &default_whitelist(),
        );

        // `a` now has two routes, `b` is untouched.
        assert_eq!(table.len(), 3);
        assert_eq!(table.resolve(ip("10.1.0.1")), Some("nodekey:a"));
        assert_eq!(table.resolve(ip("10.4.0.1")), Some("nodekey:a"));
        assert_eq!(table.resolve(ip("10.2.0.1")), Some("nodekey:b"));

        // Running the same change again must not duplicate.
        table.apply_changed(
            &[node("nodekey:a", &["10.1.0.0/16", "10.4.0.0/16"])],
            &default_whitelist(),
        );
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn rebuild_fully_replaces_previous_state() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[
                node("nodekey:a", &["10.1.0.0/16"]),
                node("nodekey:b", &["10.2.0.0/16"]),
            ],
            &default_whitelist(),
        );
        assert_eq!(table.len(), 2);

        // New netmap — `a` is gone, only `c` remains.
        table.rebuild(&[node("nodekey:c", &["10.3.0.0/16"])], &default_whitelist());
        assert_eq!(table.len(), 1);
        assert_eq!(table.resolve(ip("10.1.0.1")), None);
        assert_eq!(table.resolve(ip("10.2.0.1")), None);
        assert_eq!(table.resolve(ip("10.3.0.1")), Some("nodekey:c"));
    }

    #[test]
    fn ipv6_resolves_correctly() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[node("nodekey:v6", &["fd7a:115c:a1e0:1::/64"])],
            &default_whitelist(),
        );
        assert_eq!(
            table.resolve(ip("fd7a:115c:a1e0:1::42")),
            Some("nodekey:v6")
        );
        assert_eq!(table.resolve(ip("fd7a:115c:a1e0:2::1")), None);
    }

    #[test]
    fn ipv6_rejected_outside_whitelist() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[node("nodekey:v6", &["2001:db8::/32"])],
            &default_whitelist(),
        );
        assert_eq!(table.resolve(ip("2001:db8::1")), None);
        assert!(table.is_empty());
    }

    #[test]
    fn malformed_cidrs_are_skipped_not_fatal() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[node(
                "nodekey:p",
                &["not-a-cidr", "10.1.0.0/16", "10.2.0.0/99", ""],
            )],
            &default_whitelist(),
        );
        // Only the one good CIDR survived.
        assert_eq!(table.len(), 1);
        assert_eq!(table.resolve(ip("10.1.0.5")), Some("nodekey:p"));
    }

    #[test]
    fn routes_iter_reports_entries_longest_first() {
        let mut table = RouteTable::new();
        table.rebuild(
            &[
                node("nodekey:broad", &["10.0.0.0/8"]),
                node("nodekey:narrow", &["10.42.0.0/16"]),
            ],
            &default_whitelist(),
        );
        let collected: Vec<(IpNet, String)> =
            table.routes().map(|(n, k)| (n, k.to_string())).collect();
        assert_eq!(collected.len(), 2);
        // Longest prefix first.
        assert_eq!(collected[0].0, net("10.42.0.0/16"));
        assert_eq!(collected[0].1, "nodekey:narrow");
        assert_eq!(collected[1].0, net("10.0.0.0/8"));
        assert_eq!(collected[1].1, "nodekey:broad");
    }

    #[test]
    fn apply_changed_on_empty_table_inserts() {
        let mut table = RouteTable::new();
        table.apply_changed(
            &[node("nodekey:new", &["10.5.0.0/16"])],
            &default_whitelist(),
        );
        assert_eq!(table.resolve(ip("10.5.0.1")), Some("nodekey:new"));
    }

    #[test]
    fn peer_with_no_allowed_ips_is_silent_noop() {
        let mut table = RouteTable::new();
        table.rebuild(&[node("nodekey:empty", &[])], &default_whitelist());
        assert!(table.is_empty());
    }

    #[test]
    fn is_subnet_of_mixed_families_rejected() {
        // Sanity: v4 inside v6 whitelist (or vice versa) must not match.
        assert!(!is_subnet_of(&net("10.0.0.0/8"), &net("fd7a::/16")));
        assert!(!is_subnet_of(&net("fd7a::/16"), &net("10.0.0.0/8")));
    }
}
