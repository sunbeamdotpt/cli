use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use ipnet::IpNet;

/// Default allow-listed destination ports for the SOCKS5 proxy.
///
/// Any CONNECT to a port outside this set is rejected regardless of the
/// destination IP's presence in the route table. The list matches the
/// services Sunbeam actually needs to reach through the tunnel; tighten
/// further in Phase 6 by deriving from the service registry.
pub fn default_socks_allow_ports() -> Vec<u16> {
    vec![
        53,   // DNS (CoreDNS over cluster)
        80,   // HTTP
        443,  // HTTPS
        3000, // Grafana
        5432, // PostgreSQL
        6379, // Valkey / Redis
        6443, // kube-apiserver
        8080, // misc HTTP (SeaweedFS volume, many defaults)
        8081, // LiveKit HTTP / misc admin
        8200, // OpenBao
        8333, // SeaweedFS S3 API
        8443, // misc HTTPS
        8888, // SeaweedFS filer HTTP
        9090, // Prometheus
        9091, // SeaweedFS metrics / generic Prometheus scrape
        9153, // CoreDNS metrics
        9200, // OpenSearch REST
        9300, // OpenSearch transport
        9333, // SeaweedFS master HTTP
    ]
}

/// Default CIDR whitelist used when a caller doesn't set one explicitly.
/// Covers RFC1918, CGNAT / tailnet space, and the Tailscale ULA range.
///
/// Any advertised prefix that is not strictly contained in one of these
/// is rejected by [`crate::control::RouteTable`]. We do NOT clip — a
/// permitted route must be a full subnet of an entry here or it's dropped.
pub fn default_route_whitelist() -> Vec<IpNet> {
    [
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "100.64.0.0/10",
        "fd7a:115c:a1e0::/48",
    ]
    .iter()
    .map(|s| s.parse().expect("static default whitelist entry"))
    .collect()
}

/// Top-level configuration for a sunbeam-net VPN instance.
#[derive(Debug, Clone)]
pub struct VpnConfig {
    /// URL of the Headscale/Tailscale coordination server.
    pub coordination_url: String,
    /// Pre-auth key for automatic registration.
    pub auth_key: String,
    /// Directory for persisting keys and state.
    pub state_dir: PathBuf,
    /// Address to bind the SOCKS/TCP proxy on.
    pub proxy_bind: SocketAddr,
    /// Explicit cluster API server IP (inside the VPN). Used as a
    /// last-resort fallback when neither `cluster_api_host` resolves in
    /// the netmap nor a non-self peer address can be auto-selected.
    /// Set to `None` to let the daemon auto-pick the first non-self peer,
    /// which is correct for most single-cluster deployments.
    pub cluster_api_addr: Option<IpAddr>,
    /// Cluster API server port.
    pub cluster_api_port: u16,
    /// Optional peer hostname (or hostname prefix) to look up in the
    /// netmap for the cluster API server's tailnet IP. When set, the
    /// daemon resolves this from the first netmap and overrides
    /// `cluster_api_addr` with the matching peer's address. Set this
    /// when the cluster API runs on a node whose tailnet IP isn't
    /// known statically.
    pub cluster_api_host: Option<String>,
    /// Path for the daemon control socket.
    pub control_socket: PathBuf,
    /// Hostname to register with the coordination server.
    pub hostname: String,
    /// The coordination server's Noise public key (32 bytes).
    /// If `None`, it will be fetched from the server's `/key` endpoint.
    pub server_public_key: Option<[u8; 32]>,
    /// Skip TLS certificate verification when connecting to DERP relays
    /// over `https://`. Only set this for test stacks with self-signed
    /// certs — in production, leave it `false`.
    pub derp_tls_insecure: bool,
    /// CIDR whitelist bounding which routes may be accepted from peers.
    /// Any advertised prefix not contained in one of these is rejected.
    /// Defaults to standard RFC1918 + tailnet + ULA ranges — see
    /// [`default_route_whitelist`].
    pub route_whitelist: Vec<IpNet>,
    /// Bind address for the SOCKS5 + HTTP CONNECT proxy. Always bind to
    /// a loopback address — the daemon refuses to start if this is not
    /// `127.0.0.1` or `::1`. The port is always assigned ephemerally and
    /// written to `{state_dir}/socks5.port` (mode 0600) so the CLI can
    /// discover it. Users don't configure the port directly.
    pub socks_bind: IpAddr,
    /// Allow-listed destination ports for the SOCKS5 proxy. A CONNECT
    /// to any other port is rejected with `CONNECTION_NOT_ALLOWED` even
    /// if the destination IP is reachable via the route table. See
    /// [`default_socks_allow_ports`].
    pub socks_allow_ports: Vec<u16>,
    /// Cluster DNS server address (typically CoreDNS, e.g.
    /// `10.43.0.10:53` on k3s). When set, the SOCKS proxy accepts
    /// domain-name destinations and resolves them through the VPN
    /// via DNS-over-TCP. When `None`, domain-name CONNECTs are
    /// rejected — only literal IPs are allowed.
    pub dns_server: Option<SocketAddr>,
    /// DNS search domains appended (in order) to bare names that
    /// have no dot. Matches `resolv.conf`'s `search` directive
    /// under `ndots:1` semantics. Typically
    /// `["svc.cluster.local", "cluster.local"]`.
    pub dns_search_domains: Vec<String>,
}
