//! Network interface monitor.
//!
//! Detects IP address changes, interface up/down events, and default route
//! changes. Notifies subscribers via a [`tokio::sync::broadcast`] channel.

pub mod state;

#[cfg(target_os = "macos")]
mod darwin;

#[cfg(target_os = "linux")]
mod linux;

pub use state::{ChangeDelta, InterfaceInfo, InterfaceState};

use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Network interface monitor.
///
/// Listens for kernel route/address change notifications and broadcasts
/// [`ChangeDelta`] values to all subscribers when something meaningful changes.
pub struct Monitor {
    state: Arc<RwLock<InterfaceState>>,
    change_tx: broadcast::Sender<ChangeDelta>,
    cancel: CancellationToken,
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl Monitor {
    /// Create and start the monitor.
    pub fn new(cancel: CancellationToken) -> std::io::Result<Self> {
        let initial = InterfaceState::snapshot()?;
        let state = Arc::new(RwLock::new(initial));

        #[cfg(target_os = "macos")]
        let (change_tx, thread) = {
            let (tx, handle) = darwin::spawn_monitor(Arc::clone(&state), cancel.clone());
            (tx, Some(handle))
        };

        #[cfg(target_os = "linux")]
        let (change_tx, thread) = {
            let (tx, handle) = linux::spawn_monitor(Arc::clone(&state), cancel.clone());
            (tx, Some(handle))
        };

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let (change_tx, thread) = {
            let (tx, _) = broadcast::channel(16);
            tracing::warn!("netmon: network monitoring not implemented for this platform");
            (tx, None)
        };

        Ok(Self {
            state,
            change_tx,
            cancel,
            _thread: thread,
        })
    }

    /// Subscribe to change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<ChangeDelta> {
        self.change_tx.subscribe()
    }

    /// Get the current interface state snapshot.
    pub fn current_state(&self) -> InterfaceState {
        self.state.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Give the background thread a moment to exit.
        if let Some(handle) = self._thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_creation() {
        let cancel = CancellationToken::new();
        let monitor = Monitor::new(cancel.clone()).expect("Monitor::new should succeed");
        let state = monitor.current_state();
        assert!(!state.interfaces.is_empty());
        cancel.cancel();
    }

    #[test]
    fn test_subscribe() {
        let cancel = CancellationToken::new();
        let monitor = Monitor::new(cancel.clone()).expect("Monitor::new should succeed");
        let _rx = monitor.subscribe();
        // No panic — channel is live.
        cancel.cancel();
    }
}
