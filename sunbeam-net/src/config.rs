use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

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
    /// Cluster API server IP (inside the VPN). Used as a fallback when
    /// `cluster_api_host` is None or doesn't resolve from the netmap.
    pub cluster_api_addr: IpAddr,
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
}
