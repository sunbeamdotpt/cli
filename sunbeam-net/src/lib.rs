//! sunbeam-net: Pure Rust Headscale/Tailscale-compatible VPN client.

pub mod config;
pub mod control;
pub mod derp;
pub mod error;
pub mod keys;
pub mod noise;
pub mod proxy;
pub mod wg;
pub(crate) mod proto;

pub use config::VpnConfig;
pub use error::{Error, Result};
