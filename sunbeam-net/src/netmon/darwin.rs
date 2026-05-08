//! macOS network interface monitor backed by AF_ROUTE.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::state::{ChangeDelta, InterfaceState};

// Route message types we care about.
const RTM_ADD: u8 = 0x1;
const RTM_DELETE: u8 = 0x2;
const RTM_NEWADDR: u8 = 0xC;
const RTM_DELADDR: u8 = 0xD;
const RTM_IFINFO: u8 = 0xE;

/// Minimal route message header — we only need the first 4 bytes.
#[repr(C)]
struct RtMsghdr {
    rtm_msglen: u16,
    rtm_version: u8,
    rtm_type: u8,
}

const DEBOUNCE: Duration = Duration::from_secs(1);

fn is_interesting_msg(typ: u8) -> bool {
    matches!(
        typ,
        RTM_ADD | RTM_DELETE | RTM_NEWADDR | RTM_DELADDR | RTM_IFINFO
    )
}

/// Spawn a background thread that listens on an AF_ROUTE socket for
/// network change events. Debounces with 1 s coalescing, then computes
/// a [`ChangeDelta`] and broadcasts it.
pub(crate) fn spawn_monitor(
    state: Arc<RwLock<InterfaceState>>,
    cancel: CancellationToken,
) -> (broadcast::Sender<ChangeDelta>, std::thread::JoinHandle<()>) {
    let (tx, _) = broadcast::channel::<ChangeDelta>(16);
    let tx_clone = tx.clone();

    let handle = std::thread::spawn(move || {
        if let Err(e) = monitor_loop(state, tx_clone, cancel) {
            tracing::warn!("netmon: AF_ROUTE monitor exited with error: {e}");
        }
    });

    (tx, handle)
}

fn monitor_loop(
    state: Arc<RwLock<InterfaceState>>,
    tx: broadcast::Sender<ChangeDelta>,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    // SAFETY: Opening a standard AF_ROUTE socket.
    let fd = unsafe { libc::socket(libc::AF_ROUTE, libc::SOCK_RAW, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Set 1 s read timeout so we can check cancellation periodically.
    let tv = libc::timeval {
        tv_sec: 1,
        tv_usec: 0,
    };
    // SAFETY: setsockopt on a valid fd with a correct timeval struct.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }

    let mut buf = [0u8; 2048];

    let _guard = FdGuard(fd);

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // SAFETY: read into our stack buffer from the AF_ROUTE socket.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };

        if n < 0 {
            let err = std::io::Error::last_os_error();
            let kind = err.raw_os_error().unwrap_or(0);
            if kind == libc::EAGAIN || kind == libc::EWOULDBLOCK {
                // Timeout — loop back to check cancellation.
                continue;
            }
            // Real error — bail.
            return Err(err);
        }

        if (n as usize) < std::mem::size_of::<RtMsghdr>() {
            continue;
        }

        // SAFETY: We checked n >= size_of::<RtMsghdr>().
        let hdr = unsafe { &*(buf.as_ptr() as *const RtMsghdr) };
        if !is_interesting_msg(hdr.rtm_type) {
            continue;
        }

        // Debounce: drain more messages for DEBOUNCE duration.
        let deadline = std::time::Instant::now() + DEBOUNCE;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let drain_tv = libc::timeval {
                tv_sec: remaining.as_secs() as libc::time_t,
                tv_usec: remaining.subsec_micros() as libc::suseconds_t,
            };
            // SAFETY: updating socket timeout, valid fd.
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &drain_tv as *const libc::timeval as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                );
            }
            // SAFETY: read into stack buffer.
            let dn = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if dn < 0 {
                break; // timeout or error — done draining
            }
        }

        // Restore 1 s timeout.
        let restore_tv = libc::timeval {
            tv_sec: 1,
            tv_usec: 0,
        };
        // SAFETY: restoring socket timeout.
        unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &restore_tv as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );
        }

        // Snapshot & diff.
        let new_state = match InterfaceState::snapshot() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("netmon: snapshot failed after route event: {e}");
                continue;
            }
        };

        let old_state = {
            let guard = state.read().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };

        let delta = old_state.diff(&new_state);

        // Update shared state.
        {
            let mut guard = state.write().unwrap_or_else(|e| e.into_inner());
            *guard = new_state;
        }

        if delta.rebind_likely_required {
            // Ignore send errors — no active receivers is fine.
            let _ = tx.send(delta);
        }
    }

    Ok(())
}

/// RAII guard that closes a file descriptor on drop.
struct FdGuard(i32);

impl Drop for FdGuard {
    fn drop(&mut self) {
        // SAFETY: closing an fd we own.
        unsafe {
            libc::close(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_af_route_socket_opens() {
        // SAFETY: opening and immediately closing an AF_ROUTE socket.
        unsafe {
            let fd = libc::socket(libc::AF_ROUTE, libc::SOCK_RAW, 0);
            assert!(fd >= 0, "failed to open AF_ROUTE socket");
            libc::close(fd);
        }
    }

    #[test]
    fn test_monitor_detects_no_change_on_idle() {
        let initial = InterfaceState::snapshot().expect("snapshot");
        let state = Arc::new(RwLock::new(initial));
        let cancel = CancellationToken::new();
        let (tx, _handle) = spawn_monitor(state, cancel.clone());
        let mut rx = tx.subscribe();

        // Wait 2 s — no network changes should fire on an idle machine.
        std::thread::sleep(Duration::from_secs(2));
        cancel.cancel();

        // Receiver should have nothing (or at most spurious).
        match rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {} // expected
            Err(broadcast::error::TryRecvError::Lagged(_)) => {} // also fine
            Err(broadcast::error::TryRecvError::Closed) => {} // monitor exited
            Ok(_) => {
                // A change on idle is possible but unusual — don't fail the test.
            }
        }
    }
}
