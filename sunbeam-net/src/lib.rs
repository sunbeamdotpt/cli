//! sunbeam-net: Pure Rust Headscale/Tailscale-compatible VPN client.

pub mod config;
pub mod control;
pub mod daemon;
pub mod derp;
pub mod discovery;
pub mod disco;
pub(crate) mod dns;
pub mod error;
pub mod keys;
pub mod netmon;
pub mod noise;
pub(crate) mod proto;
pub mod proxy;
pub mod stun;
pub mod tls;
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
