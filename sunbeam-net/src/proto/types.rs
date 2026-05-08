//! Protocol types for the Tailscale control plane.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Deserialize a JSON field that may be `null` as an empty `Vec`.
///
/// Headscale sometimes serializes empty lists as JSON `null` (e.g. a
/// `FilterRule` with no source IP restrictions emits `"SrcIPs":null`).
/// `#[serde(default)]` on the struct only fills in *missing* fields, so
/// explicit null still fails without this helper.
fn null_as_empty_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// Registration request sent to POST /machine/register.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RegisterRequest {
    /// Version.
    pub version: u16,
    /// Node key.
    pub node_key: String,
    /// Old node key.
    pub old_node_key: String,
    /// Curve25519 disco public key. Headscale persists this on the node
    /// record and uses it for peer-to-peer discovery — if it's zero, peers
    /// won't include us in their netmaps.
    pub disco_key: String,
    /// Auth.
    pub auth: Option<AuthInfo>,
    /// Hostinfo.
    pub hostinfo: HostInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Followup.
    pub followup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Timestamp.
    pub timestamp: Option<String>,
}

/// Authentication metadata sent during node registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Auth key.
    pub auth_key: Option<String>,
}

/// Platform and network metadata advertised by a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct HostInfo {
    #[serde(rename = "GoArch")]
    /// Go arch.
    pub go_arch: String,
    #[serde(rename = "GoOS")]
    /// Go os.
    pub go_os: String,
    #[serde(rename = "GoVersion")]
    /// Go version.
    pub go_version: String,
    /// Hostname.
    pub hostname: String,
    #[serde(rename = "OS")]
    /// Os.
    pub os: String,
    #[serde(rename = "OSVersion")]
    /// Os version.
    pub os_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Device model.
    pub device_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Frontend log id.
    pub frontend_log_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Backend log id.
    pub backend_log_id: Option<String>,
    /// NetInfo carries DERP preferences and NAT-traversal hints. Headscale
    /// derives our Node.DERP field from `NetInfo.PreferredDERP`, and peers
    /// use that value to decide which relay region to address packets to.
    /// Without it, peers see us as `127.3.3.40:0` and DERP-only sends get
    /// dropped by the relay.
    #[serde(rename = "NetInfo", skip_serializing_if = "Option::is_none")]
    pub net_info: Option<NetInfo>,
}

/// Network/DERP information announced to the coordination server. Only the
/// field we actually care about — `PreferredDERP` — is populated; the rest
/// round-trip as `None` / defaults and Headscale accepts a partial struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct NetInfo {
    /// DERP region ID we want peers to use when relaying packets to us.
    /// Zero means "no preference", which is how Headscale interprets an
    /// absent `NetInfo` — and also how peers refuse to relay to us.
    #[serde(rename = "PreferredDERP")]
    pub preferred_derp: u32,
}

/// Registration response from POST /machine/register.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RegisterResponse {
    #[serde(default)]
    /// User.
    pub user: User,
    #[serde(default)]
    /// Login.
    pub login: Login,
    #[serde(default)]
    /// Node key expired.
    pub node_key_expired: bool,
    #[serde(default)]
    /// Machine authorized.
    pub machine_authorized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Auth url.
    pub auth_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Error.
    pub error: Option<String>,
}

/// Tailscale user record returned by the coordination server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    #[serde(rename = "ID", default)]
    /// Id.
    pub id: u64,
    #[serde(default)]
    /// Login name.
    pub login_name: String,
    #[serde(default)]
    /// Display name.
    pub display_name: String,
}

/// Tailscale login identity tied to a node registration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Login {
    #[serde(rename = "ID", default)]
    /// Id.
    pub id: u64,
    #[serde(default)]
    /// Login name.
    pub login_name: String,
    #[serde(default)]
    /// Display name.
    pub display_name: String,
}

/// Map request sent to POST /machine/map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapRequest {
    /// Version.
    pub version: u16,
    /// Node key.
    pub node_key: String,
    /// Disco key.
    pub disco_key: String,
    /// Stream.
    pub stream: bool,
    /// "Lite update" flag — set together with `Stream: false` and
    /// `ReadOnly: false` to make Headscale persist DiscoKey + endpoints
    /// without sending a full netmap response (the "Lite endpoint update"
    /// path in Headscale's poll.go).
    #[serde(default)]
    pub omit_peers: bool,
    #[serde(default)]
    /// Read only.
    pub read_only: bool,
    /// Hostinfo.
    pub hostinfo: HostInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Endpoints.
    pub endpoints: Option<Vec<String>>,
}

/// Map response -- can be a full snapshot or a delta update.
/// Fields are all optional because deltas only include changed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Node.
    pub node: Option<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Peers.
    pub peers: Option<Vec<Node>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Peers changed.
    pub peers_changed: Option<Vec<Node>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Peers removed.
    pub peers_removed: Option<Vec<String>>,
    #[serde(rename = "DERPMap")]
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Derp map.
    pub derp_map: Option<DerpMap>,
    #[serde(rename = "DNSConfig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Dns config.
    pub dns_config: Option<DnsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Packet filter.
    pub packet_filter: Option<Vec<FilterRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Domain.
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Collection name.
    pub collection_name: Option<String>,
}

/// A peer node in the tailnet with addresses, endpoints, and keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Node {
    #[serde(rename = "ID")]
    /// Id.
    pub id: u64,
    /// Key.
    pub key: String,
    /// Disco key.
    pub disco_key: String,
    #[serde(deserialize_with = "null_as_empty_vec")]
    /// Addresses.
    pub addresses: Vec<String>,
    #[serde(rename = "AllowedIPs", deserialize_with = "null_as_empty_vec")]
    /// Allowed ips.
    pub allowed_ips: Vec<String>,
    #[serde(deserialize_with = "null_as_empty_vec")]
    /// Endpoints.
    pub endpoints: Vec<String>,
    #[serde(rename = "DERP")]
    /// Derp.
    pub derp: String,
    /// Hostinfo.
    pub hostinfo: HostInfo,
    /// Name.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Online.
    pub online: Option<bool>,
    /// Machine authorized.
    pub machine_authorized: bool,
}

/// Map of DERP relay regions available to the tailnet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DerpMap {
    /// Regions.
    pub regions: HashMap<String, DerpRegion>,
}

/// A geographic DERP region containing one or more relay nodes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DerpRegion {
    #[serde(rename = "RegionID")]
    /// Region id.
    pub region_id: u16,
    /// Region code.
    pub region_code: String,
    /// Region name.
    pub region_name: String,
    /// Nodes.
    pub nodes: Vec<DerpNode>,
}

/// A single DERP relay node with IP, port, and region info.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DerpNode {
    /// Name.
    pub name: String,
    #[serde(rename = "RegionID")]
    /// Region id.
    pub region_id: u16,
    /// Host name.
    pub host_name: String,
    #[serde(rename = "IPv4")]
    /// Ipv4.
    pub ipv4: String,
    #[serde(rename = "IPv6")]
    /// Ipv6.
    pub ipv6: String,
    #[serde(rename = "DERPPort", alias = "DerpPort")]
    /// Derp port.
    pub derp_port: u16,
    #[serde(rename = "STUNPort", alias = "StunPort")]
    /// Stun port.
    pub stun_port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Stun only.
    pub stun_only: Option<bool>,
}

/// Tailnet DNS configuration (resolvers and search domains).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DnsConfig {
    #[serde(deserialize_with = "null_as_empty_vec")]
    /// Resolvers.
    pub resolvers: Vec<DnsResolver>,
    #[serde(deserialize_with = "null_as_empty_vec")]
    /// Domains.
    pub domains: Vec<String>,
}

/// A DNS resolver address advertised by the coordination server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DnsResolver {
    /// Addr.
    pub addr: String,
}

/// A packet filter rule restricting source IPs and destination ports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FilterRule {
    #[serde(rename = "SrcIPs", deserialize_with = "null_as_empty_vec")]
    /// Src ips.
    pub src_ips: Vec<String>,
    #[serde(rename = "DstPorts", deserialize_with = "null_as_empty_vec")]
    /// Dst ports.
    pub dst_ports: Vec<FilterPort>,
}

/// Destination IP and port range allowed by a filter rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FilterPort {
    #[serde(rename = "IP")]
    /// Ip.
    pub ip: String,
    /// Ports.
    pub ports: PortRange,
}

/// Inclusive port range (first .. last).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PortRange {
    /// First.
    pub first: u16,
    /// Last.
    pub last: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hostinfo() -> HostInfo {
        HostInfo {
            go_arch: "arm64".into(),
            go_os: "linux".into(),
            go_version: "sunbeam-net/0.1".into(),
            hostname: "myhost".into(),
            os: "linux".into(),
            os_version: "6.1".into(),
            device_model: None,
            frontend_log_id: None,
            backend_log_id: None,
            net_info: None,
        }
    }

    #[test]
    fn test_register_request_serialize() {
        let req = RegisterRequest {
            version: crate::CURRENT_CAP_VER,
            node_key: "nodekey:aabb".into(),
            old_node_key: "".into(),
            disco_key: "discokey:ccdd".into(),
            auth: Some(AuthInfo {
                auth_key: Some("tskey-abc".into()),
            }),
            hostinfo: sample_hostinfo(),
            followup: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        // Verify PascalCase keys
        assert!(json.contains("\"Version\""));
        assert!(json.contains("\"NodeKey\""));
        assert!(json.contains("\"OldNodeKey\""));
        assert!(json.contains("\"Hostinfo\""));
        assert!(json.contains("\"GoArch\""));
        assert!(json.contains("\"GoOS\""));
        assert!(json.contains("\"GoVersion\""));
        assert!(json.contains("\"AuthKey\""));
        // Optional None fields should be absent
        assert!(!json.contains("\"Followup\""));
        assert!(!json.contains("\"Timestamp\""));
    }

    #[test]
    fn test_register_response_deserialize() {
        let json = r#"{
            "User": {"ID": 1, "LoginName": "user@example.com", "DisplayName": "User"},
            "Login": {"ID": 2, "LoginName": "user@example.com", "DisplayName": "User"},
            "NodeKeyExpired": false,
            "MachineAuthorized": true,
            "AuthUrl": "https://login.example.com/a/xyz"
        }"#;
        let resp: RegisterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.user.id, 1);
        assert_eq!(resp.login.id, 2);
        assert!(!resp.node_key_expired);
        assert!(resp.machine_authorized);
        assert_eq!(
            resp.auth_url.as_deref(),
            Some("https://login.example.com/a/xyz")
        );
    }

    #[test]
    fn test_map_response_full_snapshot() {
        let json = r#"{
            "Node": {
                "ID": 1,
                "Key": "nodekey:aa",
                "DiscoKey": "discokey:bb",
                "Addresses": ["100.64.0.1/32"],
                "AllowedIPs": ["100.64.0.0/10"],
                "Endpoints": [],
                "DERP": "127.3.3.40:1",
                "Hostinfo": {
                    "GoArch": "arm64", "GoOS": "linux", "GoVersion": "sunbeam-net/0.1",
                    "Hostname": "self", "OS": "linux", "OSVersion": "6.1"
                },
                "Name": "self.example.com",
                "Online": true,
                "MachineAuthorized": true
            },
            "Peers": [{
                "ID": 2,
                "Key": "nodekey:cc",
                "DiscoKey": "discokey:dd",
                "Addresses": ["100.64.0.2/32"],
                "AllowedIPs": ["100.64.0.2/32"],
                "DERP": "127.3.3.40:1",
                "Hostinfo": {
                    "GoArch": "amd64", "GoOS": "linux", "GoVersion": "sunbeam-net/0.1",
                    "Hostname": "peer", "OS": "linux", "OSVersion": "6.1"
                },
                "Name": "peer.example.com",
                "Online": true,
                "MachineAuthorized": true
            }],
            "DERPMap": {
                "Regions": {
                    "1": {
                        "RegionId": 1,
                        "RegionCode": "default",
                        "RegionName": "Default",
                        "Nodes": [{
                            "Name": "1a",
                            "RegionId": 1,
                            "HostName": "derp.example.com",
                            "IPv4": "1.2.3.4",
                            "IPv6": "::1",
                            "DerpPort": 443,
                            "StunPort": 3478
                        }]
                    }
                }
            },
            "DNSConfig": {
                "Resolvers": [{"Addr": "100.64.0.1"}],
                "Domains": ["example.com"]
            },
            "Domain": "example.com"
        }"#;
        let resp: MapResponse = serde_json::from_str(json).unwrap();
        assert!(resp.node.is_some());
        let peers = resp.peers.as_ref().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].key, "nodekey:cc");
        let derp_map = resp.derp_map.as_ref().unwrap();
        assert!(derp_map.regions.contains_key("1"));
        let dns = resp.dns_config.as_ref().unwrap();
        assert_eq!(dns.resolvers.len(), 1);
        assert_eq!(dns.domains, vec!["example.com"]);
    }

    #[test]
    fn test_map_response_delta() {
        let json = r#"{
            "PeersChanged": [{
                "ID": 3,
                "Key": "nodekey:ee",
                "DiscoKey": "discokey:ff",
                "Addresses": ["100.64.0.3/32"],
                "AllowedIPs": ["100.64.0.3/32"],
                "DERP": "127.3.3.40:1",
                "Hostinfo": {
                    "GoArch": "amd64", "GoOS": "linux", "GoVersion": "sunbeam-net/0.1",
                    "Hostname": "new-peer", "OS": "linux", "OSVersion": "6.1"
                },
                "Name": "new-peer.example.com",
                "MachineAuthorized": true
            }],
            "PeersRemoved": ["nodekey:cc"]
        }"#;
        let resp: MapResponse = serde_json::from_str(json).unwrap();
        assert!(resp.node.is_none());
        assert!(resp.peers.is_none());
        let changed = resp.peers_changed.as_ref().unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].key, "nodekey:ee");
        let removed = resp.peers_removed.as_ref().unwrap();
        assert_eq!(removed, &["nodekey:cc"]);
    }

    #[test]
    fn test_map_response_keepalive() {
        let resp: MapResponse = serde_json::from_str("{}").unwrap();
        assert!(resp.node.is_none());
        assert!(resp.peers.is_none());
        assert!(resp.peers_changed.is_none());
        assert!(resp.peers_removed.is_none());
        assert!(resp.derp_map.is_none());
        assert!(resp.dns_config.is_none());
        assert!(resp.packet_filter.is_none());
        assert!(resp.domain.is_none());
        assert!(resp.collection_name.is_none());
    }

    #[test]
    fn test_node_serde_round_trip() {
        let node = Node {
            id: 42,
            key: "nodekey:abcd".into(),
            disco_key: "discokey:1234".into(),
            addresses: vec!["100.64.0.5/32".into()],
            allowed_ips: vec!["100.64.0.0/10".into()],
            endpoints: vec!["1.2.3.4:41641".into()],
            derp: "127.3.3.40:1".into(),
            hostinfo: sample_hostinfo(),
            name: "test.example.com".into(),
            online: Some(true),
            machine_authorized: true,
        };
        let json = serde_json::to_string(&node).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 42);
        assert_eq!(back.key, "nodekey:abcd");
        assert_eq!(back.disco_key, "discokey:1234");
        assert_eq!(back.derp, "127.3.3.40:1");
        assert_eq!(back.name, "test.example.com");
        assert_eq!(back.online, Some(true));
    }

    #[test]
    fn test_filter_rule_null_src_ips() {
        // Regression: Headscale emits `"SrcIPs":null` on rules that allow
        // any source. serde's struct-level `default` only fills in missing
        // fields, not explicit nulls, so we need a deserializer that
        // accepts both.
        let json = r#"{
            "PacketFilter": [{
                "SrcIPs": null,
                "DstPorts": [
                    {"IP": "10.42.0.0/16", "Ports": {"First": 0, "Last": 65535}},
                    {"IP": "10.43.0.0/16", "Ports": {"First": 0, "Last": 65535}}
                ]
            }]
        }"#;
        let resp: MapResponse = serde_json::from_str(json).unwrap();
        let rules = resp.packet_filter.as_ref().unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].src_ips.is_empty());
        assert_eq!(rules[0].dst_ports.len(), 2);
        assert_eq!(rules[0].dst_ports[0].ip, "10.42.0.0/16");
    }

    #[test]
    fn test_node_null_vec_fields() {
        // Regression: Headscale may serialize empty peer lists' Addresses
        // or Endpoints as null rather than [].
        let json = r#"{
            "ID": 5,
            "Key": "nodekey:aa",
            "DiscoKey": "discokey:bb",
            "Addresses": null,
            "AllowedIPs": null,
            "Endpoints": null,
            "DERP": "127.3.3.40:1",
            "Hostinfo": {"GoArch":"arm64","GoOS":"linux","GoVersion":"x","Hostname":"h","OS":"linux","OSVersion":"6.1"},
            "Name": "n.example.com",
            "MachineAuthorized": true
        }"#;
        let node: Node = serde_json::from_str(json).unwrap();
        assert!(node.addresses.is_empty());
        assert!(node.allowed_ips.is_empty());
        assert!(node.endpoints.is_empty());
    }

    #[test]
    fn test_hostinfo_platform_fields() {
        let hi = sample_hostinfo();
        let json = serde_json::to_string(&hi).unwrap();
        // Verify the explicit renames
        assert!(json.contains("\"GoArch\""));
        assert!(json.contains("\"GoOS\""));
        assert!(json.contains("\"GoVersion\""));
        assert!(json.contains("\"OS\""));
        assert!(json.contains("\"OSVersion\""));
        // Verify PascalCase on normal fields
        assert!(json.contains("\"Hostname\""));
        // Should not contain snake_case
        assert!(!json.contains("go_arch"));
        assert!(!json.contains("go_os"));
        assert!(!json.contains("os_version"));
    }
}
