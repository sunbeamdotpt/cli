//! Service registry — the daemon-side half of the `sunbeam.pt/*`
//! annotation story. The controller writes
//! `~/.sunbeam/vpn/services.json` atomically on every reconcile; the
//! daemon re-reads it on change and surfaces it two ways:
//!
//! * via IPC (`IpcCommand::Services`) for the desktop app's discovery UI
//! * via the SOCKS5 proxy's bare-slug pre-resolution
//!
//! See `docs/service-discovery.md` for the full schema.

pub mod registry;

pub use registry::{
    RegistryFile, RegistryWatcher, Scheme, ServiceEntry, ServiceRegistry, Tier, is_valid_slug,
};
