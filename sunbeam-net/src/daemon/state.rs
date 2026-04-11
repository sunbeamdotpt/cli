use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Operational status of the VPN daemon.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStatus {
    /// The daemon is starting up (loading keys, etc.).
    Starting,
    /// Connecting to the coordination server.
    Connecting,
    /// Performing Noise handshake with the coordination server.
    Handshaking,
    /// Registering with the coordination server.
    Registering,
    /// Fully connected and forwarding traffic.
    Running {
        /// Our assigned VPN IP addresses.
        addresses: Vec<IpAddr>,
        /// Number of connected peers.
        peer_count: usize,
        /// Home DERP region ID.
        derp_home: Option<u16>,
        /// Local loopback port the SOCKS5 + HTTP CONNECT proxy is bound to.
        /// `None` until the proxy has been staged. CLI discovers the
        /// matching auth token from `{state_dir}/socks5.auth`.
        #[serde(default)]
        socks_proxy_port: Option<u16>,
    },
    /// Reconnecting after a connection loss.
    Reconnecting {
        /// Number of reconnection attempts so far.
        attempt: u32,
    },
    /// The daemon is shutting down.
    ShuttingDown,
    /// The daemon has stopped (terminal state).
    Stopped,
    /// The daemon encountered a fatal error.
    Error { message: String },
}

impl std::fmt::Display for DaemonStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Connecting => write!(f, "connecting"),
            Self::Handshaking => write!(f, "handshaking"),
            Self::Registering => write!(f, "registering"),
            Self::Running {
                addresses,
                peer_count,
                ..
            } => {
                let addrs: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
                write!(f, "running ({}), {} peers", addrs.join(", "), peer_count)
            }
            Self::Reconnecting { attempt } => write!(f, "reconnecting (attempt {attempt})"),
            Self::ShuttingDown => write!(f, "shutting down"),
            Self::Stopped => write!(f, "stopped"),
            Self::Error { message } => write!(f, "error: {message}"),
        }
    }
}

/// Handle to a running VPN daemon for status queries and control.
pub struct DaemonHandle {
    /// Path to the daemon's control socket (for IPC-based handles).
    pub control_socket: PathBuf,
    /// Cached status (for IPC-based handles).
    pub status: DaemonStatus,
    /// Cancellation token shared with the daemon loop and the IPC server.
    /// `shutdown()` cancels it; an IPC `Stop` command also cancels it.
    shutdown: Option<CancellationToken>,
    /// Shared live status (for in-process handles).
    live_status: Option<Arc<RwLock<DaemonStatus>>>,
    /// Background task join handle.
    join: Option<JoinHandle<crate::Result<()>>>,
}

impl std::fmt::Debug for DaemonHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonHandle")
            .field("control_socket", &self.control_socket)
            .field("status", &self.status)
            .finish()
    }
}

impl Clone for DaemonHandle {
    fn clone(&self) -> Self {
        Self {
            control_socket: self.control_socket.clone(),
            status: self.status.clone(),
            shutdown: self.shutdown.clone(),
            live_status: self.live_status.clone(),
            join: None,
        }
    }
}

impl DaemonHandle {
    /// Create a new handle pointing at the given control socket (for IPC clients).
    pub fn new(control_socket: PathBuf) -> Self {
        Self {
            control_socket,
            status: DaemonStatus::Stopped,
            shutdown: None,
            live_status: None,
            join: None,
        }
    }

    /// Create a handle for an in-process daemon with shutdown and status tracking.
    pub(crate) fn with_daemon(
        shutdown: CancellationToken,
        status: Arc<RwLock<DaemonStatus>>,
        join: JoinHandle<crate::Result<()>>,
    ) -> Self {
        Self {
            control_socket: PathBuf::new(),
            status: DaemonStatus::Starting,
            shutdown: Some(shutdown),
            live_status: Some(status),
            join: Some(join),
        }
    }

    /// Query the current daemon status.
    pub fn current_status(&self) -> DaemonStatus {
        if let Some(ref live) = self.live_status
            && let Ok(s) = live.read()
        {
            return s.clone();
        }
        self.status.clone()
    }

    /// Signal the daemon to shut down and wait for it to finish.
    pub async fn shutdown(self) -> crate::Result<()> {
        if let Some(token) = self.shutdown {
            token.cancel();
        }
        if let Some(join) = self.join {
            match join.await {
                Ok(result) => result,
                Err(e) => Err(crate::Error::Daemon(format!("daemon task panicked: {e}"))),
            }
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_display() {
        assert_eq!(DaemonStatus::Starting.to_string(), "starting");
        assert_eq!(DaemonStatus::Connecting.to_string(), "connecting");

        let running = DaemonStatus::Running {
            addresses: vec!["100.64.0.1".parse().unwrap()],
            peer_count: 3,
            derp_home: Some(1),
            socks_proxy_port: Some(16580),
        };
        assert_eq!(running.to_string(), "running (100.64.0.1), 3 peers");
    }

    #[test]
    fn status_serde_round_trip() {
        let status = DaemonStatus::Running {
            addresses: vec!["fd7a:115c:a1e0::1".parse().unwrap()],
            peer_count: 5,
            derp_home: Some(2),
            socks_proxy_port: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        let deserialized: DaemonStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, deserialized);
    }

    #[test]
    fn daemon_handle_default_status() {
        let handle = DaemonHandle::new("/tmp/test.sock".into());
        assert_eq!(handle.status, DaemonStatus::Stopped);
    }
}
