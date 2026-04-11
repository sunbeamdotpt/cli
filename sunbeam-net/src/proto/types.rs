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
    pub version: u16,
    pub node_key: String,
    pub old_node_key: String,
    /// Curve25519 disco public key. Headscale persists this on the node
    /// record and uses it for peer-to-peer discovery — if it's zero, peers
    /// won't include us in their netmaps.
    pub disco_key: String,
    pub auth: Option<AuthInfo>,
    pub hostinfo: HostInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_key: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct HostInfo {
    #[serde(rename = "GoArch")]
    pub go_arch: String,
    #[serde(rename = "GoOS")]
    pub go_os: String,
    #[serde(rename = "GoVersion")]
    pub go_version: String,
    pub hostname: String,
    #[serde(rename = "OS")]
    pub os: String,
    #[serde(rename = "OSVersion")]
    pub os_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend_log_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    pub user: User,
    #[serde(default)]
    pub login: Login,
    #[serde(default)]
    pub node_key_expired: bool,
    #[serde(default)]
    pub machine_authorized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    #[serde(rename = "ID", default)]
    pub id: u64,
    #[serde(default)]
    pub login_name: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Login {
    #[serde(rename = "ID", default)]
    pub id: u64,
    #[serde(default)]
    pub login_name: String,
    #[serde(default)]
    pub display_name: String,
}

/// Map request sent to POST /machine/map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapRequest {
    pub version: u16,
    pub node_key: String,
    pub disco_key: String,
    pub stream: bool,
    /// "Lite update" flag — set together with `Stream: false` and
    /// `ReadOnly: false` to make Headscale persist DiscoKey + endpoints
    /// without sending a full netmap response (the "Lite endpoint update"
    /// path in Headscale's poll.go).
    #[serde(default)]
    pub omit_peers: bool,
    #[serde(default)]
    pub read_only: bool,
    pub hostinfo: HostInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Vec<String>>,
}

/// Map response -- can be a full snapshot or a delta update.
/// Fields are all optional because deltas only include changed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<Vec<Node>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers_changed: Option<Vec<Node>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers_removed: Option<Vec<String>>,
    #[serde(rename = "DERPMap")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derp_map: Option<DerpMap>,
    #[serde(rename = "DNSConfig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_config: Option<DnsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_filter: Option<Vec<FilterRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Node {
    #[serde(rename = "ID")]
    pub id: u64,
    pub key: String,
    pub disco_key: String,
    #[serde(deserialize_with = "null_as_empty_vec")]
    pub addresses: Vec<String>,
    #[serde(rename = "AllowedIPs", deserialize_with = "null_as_empty_vec")]
    pub allowed_ips: Vec<String>,
    #[serde(deserialize_with = "null_as_empty_vec")]
    pub endpoints: Vec<String>,
    #[serde(rename = "DERP")]
    pub derp: String,
    pub hostinfo: HostInfo,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    pub machine_authorized: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DerpMap {
    pub regions: HashMap<String, DerpRegion>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DerpRegion {
    #[serde(rename = "RegionID")]
    pub region_id: u16,
    pub region_code: String,
    pub region_name: String,
    pub nodes: Vec<DerpNode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DerpNode {
    pub name: String,
    #[serde(rename = "RegionID")]
    pub region_id: u16,
    pub host_name: String,
    #[serde(rename = "IPv4")]
    pub ipv4: String,
    #[serde(rename = "IPv6")]
    pub ipv6: String,
    pub derp_port: u16,
    pub stun_port: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stun_only: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DnsConfig {
    #[serde(deserialize_with = "null_as_empty_vec")]
    pub resolvers: Vec<DnsResolver>,
    #[serde(deserialize_with = "null_as_empty_vec")]
    pub domains: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DnsResolver {
    pub addr: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FilterRule {
    #[serde(rename = "SrcIPs", deserialize_with = "null_as_empty_vec")]
    pub src_ips: Vec<String>,
    #[serde(rename = "DstPorts", deserialize_with = "null_as_empty_vec")]
    pub dst_ports: Vec<FilterPort>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FilterPort {
    #[serde(rename = "IP")]
    pub ip: String,
    pub ports: PortRange,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PortRange {
    pub first: u16,
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
            version: 74,
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
        assert_eq!(resp.auth_url.as_deref(), Some("https://login.example.com/a/xyz"));
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
