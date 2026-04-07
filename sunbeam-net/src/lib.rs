//! sunbeam-net: Pure Rust Headscale/Tailscale-compatible VPN client.

pub mod config;
pub mod control;
pub mod daemon;
pub mod derp;
pub mod error;
pub mod keys;
pub mod noise;
pub mod proxy;
pub mod tls;
pub mod wg;
pub(crate) mod proto;

pub use config::VpnConfig;
pub use daemon::{DaemonHandle, DaemonStatus, IpcClient, IpcCommand, IpcResponse, VpnDaemon};
pub use error::{Error, Result};
