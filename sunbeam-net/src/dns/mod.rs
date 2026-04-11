//! Internal cluster DNS resolver used by the SOCKS5 proxy.
//!
//! Submodules:
//! - [`codec`]: minimal DNS wire-format encoder/decoder for A/AAAA
//! - [`resolver`]: cached resolver that runs queries through the
//!   VPN engine's TCP path

pub(crate) mod codec;
pub(crate) mod resolver;
