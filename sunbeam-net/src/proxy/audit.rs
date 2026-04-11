//! Ring-buffer audit log of recent SOCKS5 / HTTP CONNECT attempts.
//!
//! Every proxy connection — whether accepted or denied — writes one
//! [`AuditEntry`] here. `sunbeam vpn status` reads the tail via IPC so
//! the operator can see *what just happened* without tailing daemon logs.
//!
//! The buffer is a `Mutex<VecDeque>` rather than something fancier. Audit
//! writes happen at human timescale (one per client connection), reads
//! happen even less often (on operator request), and the whole thing is
//! capped at a few hundred entries — contention is not a concern.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default number of entries to retain.
pub const DEFAULT_CAPACITY: usize = 256;

/// How the proxy decided to handle a connection.
///
/// Kept as a string-tagged enum so the IPC wire format stays stable
/// even if we add new denial reasons.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Connection passed ACL and was handed to the engine.
    Accepted,
    /// Destination IP (post-resolution) was not covered by the
    /// subnet-router whitelist + peer route table.
    DeniedNotRoutable,
    /// Destination port was not in the allow-list.
    DeniedPort,
    /// A domain name was given but no DNS resolver is configured.
    DeniedNoDns,
    /// A domain name was resolved but resolution failed
    /// (NXDOMAIN, timeout, codec error, …).
    DeniedResolveFailed,
    /// Protocol-level denial that isn't tied to a specific destination
    /// (bad version byte, unsupported command, parse error, auth failure).
    DeniedProtocol,
}

impl AuditOutcome {
    /// Short one-word label for status display, matching the wire tag.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::DeniedNotRoutable => "denied/not-routable",
            Self::DeniedPort => "denied/port",
            Self::DeniedNoDns => "denied/no-dns",
            Self::DeniedResolveFailed => "denied/resolve",
            Self::DeniedProtocol => "denied/protocol",
        }
    }
}

/// One connection attempt captured for the audit trail.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    /// Seconds since the Unix epoch. Wire-safe and trivially rendered
    /// on the CLI side.
    pub unix_seconds: u64,
    /// `"socks5"` or `"http"` — the wire protocol that produced the entry.
    pub protocol: String,
    /// Human-readable destination (`host:port`, `ip:port`, `ip:port (from host)`).
    /// Deliberately free-form so we can carry the pre-resolution name.
    pub destination: String,
    /// ACL verdict + reason.
    pub outcome: AuditOutcome,
}

/// Shared ring buffer of recent audit entries.
#[derive(Debug)]
pub struct AuditLog {
    inner: Mutex<VecDeque<AuditEntry>>,
    capacity: usize,
}

impl AuditLog {
    /// Create a log with the default capacity ([`DEFAULT_CAPACITY`]).
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a log that retains at most `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
            capacity: capacity.max(1),
        }
    }

    /// Record an attempt. Never blocks and never errors — if the
    /// mutex is poisoned we log a debug line and drop the entry.
    pub fn record(&self, protocol: &str, destination: String, outcome: AuditOutcome) {
        let entry = AuditEntry {
            unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            protocol: protocol.to_string(),
            destination,
            outcome,
        };
        match self.inner.lock() {
            Ok(mut q) => {
                if q.len() == self.capacity {
                    q.pop_front();
                }
                q.push_back(entry);
            }
            Err(e) => {
                tracing::debug!("audit log mutex poisoned: {e}");
            }
        }
    }

    /// Snapshot the tail of the buffer, newest first, capped to `max`
    /// entries. Used by IPC to answer `RecentConnections`.
    pub fn snapshot(&self, max: usize) -> Vec<AuditEntry> {
        let q = match self.inner.lock() {
            Ok(q) => q,
            Err(_) => return Vec::new(),
        };
        q.iter().rev().take(max).cloned().collect()
    }

    /// Number of entries currently retained.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(s: &str) -> String {
        s.to_string()
    }

    #[test]
    fn new_log_is_empty() {
        let log = AuditLog::new();
        assert_eq!(log.len(), 0);
        assert!(log.snapshot(10).is_empty());
    }

    #[test]
    fn record_and_snapshot_newest_first() {
        let log = AuditLog::new();
        log.record("socks5", destination("10.0.0.1:443"), AuditOutcome::Accepted);
        log.record("http", destination("10.0.0.2:443"), AuditOutcome::DeniedPort);
        log.record(
            "socks5",
            destination("hydra:443"),
            AuditOutcome::DeniedNoDns,
        );

        let snap = log.snapshot(10);
        assert_eq!(snap.len(), 3);
        // Newest first.
        assert_eq!(snap[0].destination, "hydra:443");
        assert_eq!(snap[0].outcome, AuditOutcome::DeniedNoDns);
        assert_eq!(snap[1].destination, "10.0.0.2:443");
        assert_eq!(snap[2].destination, "10.0.0.1:443");
    }

    #[test]
    fn ring_buffer_evicts_oldest() {
        let log = AuditLog::with_capacity(3);
        for i in 0..5u16 {
            log.record(
                "socks5",
                format!("10.0.0.{i}:443"),
                AuditOutcome::Accepted,
            );
        }
        assert_eq!(log.len(), 3);
        let snap = log.snapshot(10);
        // Only the last three (i=2,3,4), newest first.
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].destination, "10.0.0.4:443");
        assert_eq!(snap[1].destination, "10.0.0.3:443");
        assert_eq!(snap[2].destination, "10.0.0.2:443");
    }

    #[test]
    fn capacity_zero_treated_as_one() {
        let log = AuditLog::with_capacity(0);
        log.record("socks5", destination("a:1"), AuditOutcome::Accepted);
        log.record("socks5", destination("b:2"), AuditOutcome::Accepted);
        assert_eq!(log.len(), 1);
        assert_eq!(log.snapshot(10)[0].destination, "b:2");
    }

    #[test]
    fn snapshot_honors_max() {
        let log = AuditLog::new();
        for i in 0..10u16 {
            log.record(
                "socks5",
                format!("10.0.0.{i}:443"),
                AuditOutcome::Accepted,
            );
        }
        assert_eq!(log.snapshot(3).len(), 3);
        assert_eq!(log.snapshot(100).len(), 10);
        assert_eq!(log.snapshot(0).len(), 0);
    }

    #[test]
    fn outcome_serde_round_trip() {
        let variants = [
            AuditOutcome::Accepted,
            AuditOutcome::DeniedNotRoutable,
            AuditOutcome::DeniedPort,
            AuditOutcome::DeniedNoDns,
            AuditOutcome::DeniedResolveFailed,
            AuditOutcome::DeniedProtocol,
        ];
        for v in variants {
            let j = serde_json::to_string(&v).unwrap();
            let back: AuditOutcome = serde_json::from_str(&j).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn entry_serde_round_trip() {
        let e = AuditEntry {
            unix_seconds: 1_700_000_000,
            protocol: "socks5".to_string(),
            destination: "10.42.0.1:443".into(),
            outcome: AuditOutcome::Accepted,
        };
        let j = serde_json::to_string(&e).unwrap();
        let back: AuditEntry = serde_json::from_str(&j).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn outcome_label_matches_variant() {
        assert_eq!(AuditOutcome::Accepted.label(), "accepted");
        assert_eq!(AuditOutcome::DeniedPort.label(), "denied/port");
        assert_eq!(AuditOutcome::DeniedNotRoutable.label(), "denied/not-routable");
        assert_eq!(AuditOutcome::DeniedNoDns.label(), "denied/no-dns");
        assert_eq!(AuditOutcome::DeniedResolveFailed.label(), "denied/resolve");
        assert_eq!(AuditOutcome::DeniedProtocol.label(), "denied/protocol");
    }
}
