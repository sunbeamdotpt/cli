//! VPN daemon — endpoint lifecycle, IPC, and state management.

#[doc(hidden)]
pub mod endpoint;
/// Ipc.
pub mod ipc;
/// Lifecycle.
pub mod lifecycle;
/// State.
pub mod state;

pub use ipc::{IpcClient, IpcCommand, IpcResponse, RouteInfo};
pub use lifecycle::VpnDaemon;
pub use state::{DaemonHandle, DaemonStatus};
