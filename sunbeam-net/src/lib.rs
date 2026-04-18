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
