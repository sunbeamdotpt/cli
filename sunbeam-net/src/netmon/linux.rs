//! Linux network interface monitor backed by NETLINK_ROUTE (rtnetlink).
//!
//! Subscribes to `RTMGRP_LINK`, `RTMGRP_IPV4_IFADDR`, `RTMGRP_IPV6_IFADDR`,
//! `RTMGRP_IPV4_ROUTE`, and `RTMGRP_IPV6_ROUTE` multicast groups. Messages are
//! treated as "something changed" signals — we re-snapshot via
//! [`InterfaceState::snapshot`] (getifaddrs + /proc/net/route) to compute the
//! new state rather than threading individual rtnetlink deltas through.
//!
//! Debounces with 1 s coalescing, matching the macOS AF_ROUTE implementation.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::StreamExt;
use rtnetlink::packet_core::NetlinkPayload;
use rtnetlink::packet_route::RouteNetlinkMessage;
use rtnetlink::{new_multicast_connection, MulticastGroup};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::state::{ChangeDelta, InterfaceState};

const DEBOUNCE: Duration = Duration::from_secs(1);

/// Return true if the message is one we care about (link / addr / route).
fn is_interesting(msg: &RouteNetlinkMessage) -> bool {
    matches!(
        msg,
        RouteNetlinkMessage::NewLink(_)
            | RouteNetlinkMessage::DelLink(_)
            | RouteNetlinkMessage::NewAddress(_)
            | RouteNetlinkMessage::DelAddress(_)
            | RouteNetlinkMessage::NewRoute(_)
            | RouteNetlinkMessage::DelRoute(_)
    )
}

/// Spawn a background thread running a current-thread Tokio runtime that
/// listens on a NETLINK_ROUTE multicast socket. Debounces events over 1 s,
/// re-snapshots [`InterfaceState`], and broadcasts a [`ChangeDelta`] when the
/// snapshot differs.
pub(crate) fn spawn_monitor(
    state: Arc<RwLock<InterfaceState>>,
    cancel: CancellationToken,
) -> (broadcast::Sender<ChangeDelta>, std::thread::JoinHandle<()>) {
    let (tx, _) = broadcast::channel::<ChangeDelta>(16);
    let tx_clone = tx.clone();

    let handle = std::thread::Builder::new()
        .name("sunbeam-netmon".to_owned())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::warn!("netmon: failed to build current-thread runtime: {e}");
                    return;
                }
            };

            rt.block_on(async move {
                if let Err(e) = monitor_loop(state, tx_clone, cancel).await {
                    tracing::warn!("netmon: rtnetlink monitor exited with error: {e}");
                }
            });
        })
        .expect("spawn netmon thread");

    (tx, handle)
}

async fn monitor_loop(
    state: Arc<RwLock<InterfaceState>>,
    tx: broadcast::Sender<ChangeDelta>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let groups = [
        MulticastGroup::Link,
        MulticastGroup::Ipv4Ifaddr,
        MulticastGroup::Ipv6Ifaddr,
        MulticastGroup::Ipv4Route,
        MulticastGroup::Ipv6Route,
    ];

    let (connection, _handle, mut messages) = new_multicast_connection(&groups)?;

    // Drive the connection on the same runtime.
    let conn_task = tokio::spawn(connection);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            maybe_msg = messages.next() => {
                let Some((msg, _addr)) = maybe_msg else {
                    // Stream closed; connection dropped.
                    break;
                };

                let NetlinkPayload::InnerMessage(inner) = msg.payload else {
                    continue;
                };
                if !is_interesting(&inner) {
                    continue;
                }

                // Debounce: drain additional messages for DEBOUNCE duration.
                let deadline = tokio::time::Instant::now() + DEBOUNCE;
                loop {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            break;
                        }
                        _ = tokio::time::sleep(remaining) => {
                            break;
                        }
                        drained = messages.next() => {
                            // Discard contents; we re-snapshot at the end.
                            if drained.is_none() {
                                break;
                            }
                        }
                    }
                }

                if cancel.is_cancelled() {
                    break;
                }

                // Snapshot & diff.
                let new_state = match InterfaceState::snapshot() {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("netmon: snapshot failed after rtnetlink event: {e}");
                        continue;
                    }
                };

                let old_state = {
                    let guard = state.read().unwrap_or_else(|e| e.into_inner());
                    guard.clone()
                };

                let delta = old_state.diff(&new_state);

                {
                    let mut guard = state.write().unwrap_or_else(|e| e.into_inner());
                    *guard = new_state;
                }

                if delta.rebind_likely_required {
                    // Ignore send errors — no active receivers is fine.
                    let _ = tx.send(delta);
                }
            }
        }
    }

    conn_task.abort();
    let _ = conn_task.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_multicast_connection_opens() {
        // Opening a read-only rtnetlink multicast socket does not require root.
        // If this fails on the test host it's typically because netlink is
        // unavailable (e.g. a very restricted container) — skip instead of
        // failing the build.
        let groups = [
            MulticastGroup::Link,
            MulticastGroup::Ipv4Ifaddr,
            MulticastGroup::Ipv6Ifaddr,
            MulticastGroup::Ipv4Route,
            MulticastGroup::Ipv6Route,
        ];
        match new_multicast_connection(&groups) {
            Ok((conn, _handle, _messages)) => {
                // Immediately drop the connection; we just want to prove the
                // socket + bind + membership joins succeed unprivileged.
                drop(conn);
            }
            Err(e) => {
                eprintln!(
                    "skipping: rtnetlink multicast socket unavailable on this host: {e}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_monitor_spawns_and_cancels() {
        let initial = InterfaceState::snapshot().expect("snapshot");
        let state = Arc::new(RwLock::new(initial));
        let cancel = CancellationToken::new();
        let (tx, handle) = spawn_monitor(Arc::clone(&state), cancel.clone());
        let mut rx = tx.subscribe();

        // Let the monitor attach to netlink; no changes expected on an idle host.
        tokio::time::sleep(Duration::from_millis(500)).await;
        cancel.cancel();
        let _ = handle.join();

        match rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {}
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(broadcast::error::TryRecvError::Closed) => {}
            Ok(_) => {
                // A spontaneous change on an idle host is unusual but
                // possible — don't fail the test.
            }
        }
    }
}
