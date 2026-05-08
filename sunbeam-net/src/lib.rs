#![warn(missing_docs)]
//! sunbeam-net: Pure Rust Headscale/Tailscale-compatible VPN client.

/// Configuration types and environment loading for the Sunbeam network daemon.
pub mod config;
/// Control plane client — registers the node and polls for netmap updates.
pub mod control;
/// VPN daemon lifecycle, IPC, and per-endpoint state management.
pub mod daemon;
/// DERP relay client and framing — fallback path when NAT traversal fails.
pub mod derp;
/// Service registry and peer discovery for tailnet services.
pub mod discovery;
/// Disco protocol — encrypted ping/pong and endpoint exchange.
pub mod disco;
pub(crate) mod dns;
/// Error types and result aliases for sunbeam-net.
pub mod error;
/// Node key generation, persistence, and rotation.
pub mod keys;
/// Network interface monitor (Darwin / Linux).
pub mod netmon;
/// Noise protocol handshake and encrypted stream framing.
pub mod noise;
pub(crate) mod proto;
/// SOCKS5 / HTTP CONNECT proxy and audit logging.
pub mod proxy;
/// STUN binding requests for NAT detection and endpoint discovery.
pub mod stun;
/// TLS and TCP connection helpers.
pub mod tls;
/// WireGuard tunnel and platform socket bindings.
pub mod wg;

pub use config::VpnConfig;
pub use daemon::{
    DaemonHandle, DaemonStatus, IpcClient, IpcCommand, IpcResponse, RouteInfo, VpnDaemon,
};
pub use error::{Error, Result};
pub use proxy::audit::{AuditEntry, AuditOutcome};
pub use proxy::socks::SOCKS5_PORT;

/// Tailscale `CapabilityVersion` we advertise everywhere — the noise handshake
/// prologue (`noise::handshake`), the `MapRequest.Version` field
/// (`proto::types` + `control::netmap`), and the `RegisterRequest.Version`
/// field (`control::register`). Headscale rejects any value below its own
/// `MinSupportedCapabilityVersion` (109 in v0.28.0, raised whenever headscale
/// drops an older minor-version window). If this goes stale you'll see
/// `noise upgrade failed: unsupported client version` in the server log —
/// bump this constant and re-check the server's minimum.
pub const CURRENT_CAP_VER: u16 = 109;
