//! Control plane client — node registration and netmap streaming.

pub mod client;
/// Netmap.
pub mod netmap;
/// Register.
pub mod register;
/// Routes.
pub mod routes;

pub use client::ControlClient;
pub use netmap::{MapStream, MapUpdate};
pub use routes::RouteTable;
