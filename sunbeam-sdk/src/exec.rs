//! Shared helper for interactive exec-into-pod with TTY via the kube-rs API.
//!
//! Replaces `kubectl exec -it` shellouts. Handles raw-mode stdin (so arrow
//! keys, Ctrl-C, etc. reach the remote shell), pipes stdin/stdout between the
//! local terminal and the remote process, and restores cooked mode on return.

use crate::error::{Result, SunbeamError};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, TerminalSize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// RAII guard that puts the real STDIN fd into raw mode on construction and
/// restores the previous termios on drop. No-op on non-Unix targets.
#[cfg(unix)]
struct RawModeGuard {
    fd: libc::c_int,
    saved: libc::termios,
    is_tty: bool,
}

#[cfg(unix)]
impl RawModeGuard {
    fn new() -> Self {
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stdin().as_raw_fd();
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if is_tty {
            unsafe {
                libc::tcgetattr(fd, &mut saved);
                let mut raw = saved;
                libc::cfmakeraw(&mut raw);
                libc::tcsetattr(fd, libc::TCSANOW, &raw);
            }
        }
        Self { fd, saved, is_tty }
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.is_tty {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
            }
        }
    }
}

#[cfg(not(unix))]
struct RawModeGuard;

#[cfg(not(unix))]
impl RawModeGuard {
    fn new() -> Self {
        Self
    }
}

/// Return the current terminal size as (cols, rows), or None if stdout is not
/// a tty.
#[cfg(unix)]
fn terminal_size() -> Option<(u16, u16)> {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdout().as_raw_fd();
    if unsafe { libc::isatty(fd) } != 1 {
        return None;
    }
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 {
        Some((ws.ws_col, ws.ws_row))
    } else {
        None
    }
}

#[cfg(not(unix))]
fn terminal_size() -> Option<(u16, u16)> {
    None
}

/// Exec into a pod interactively with a TTY. Returns the process exit code
/// (0 on success, 1 on any non-success status).
///
/// `cmd` is the argv to run in the pod. `container` selects a specific
/// container inside the pod, or `None` for the pod's default container.
#[tracing::instrument]
pub async fn pod_exec_interactive(
    pods: &Api<Pod>,
    pod_name: &str,
    container: Option<&str>,
    cmd: &[String],
) -> Result<i32> {
    tracing::debug!("pod_exec_interactive {pod_name} container={container:?} cmd={cmd:?}");
    // stdin/stdout/tty=true, stderr=false (stderr is incompatible with tty).
    let ap = AttachParams {
        stdin: true,
        stdout: true,
        stderr: false,
        tty: true,
        container: container.map(String::from),
        ..AttachParams::default()
    };

    let mut attached = pods
        .exec(pod_name, cmd, &ap)
        .await
        .map_err(|e| SunbeamError::Other(format!("kube exec failed: {e}")))?;

    // Send initial terminal size if both sides are a TTY.
    if let (Some(mut tx), Some((cols, rows))) = (attached.terminal_size(), terminal_size()) {
        use futures::SinkExt;
        let _ = tx
            .send(TerminalSize {
                width: cols,
                height: rows,
            })
            .await;
    }

    let proc_stdin = attached
        .stdin()
        .ok_or_else(|| SunbeamError::Other("no stdin stream from exec".into()))?;
    let proc_stdout = attached
        .stdout()
        .ok_or_else(|| SunbeamError::Other("no stdout stream from exec".into()))?;

    // Raw mode — restores on drop even on panic/early return.
    let _raw = RawModeGuard::new();

    let stdin_task = tokio::spawn(pipe_stdin(proc_stdin));
    let stdout_task = tokio::spawn(pipe_stdout(proc_stdout));

    // Wait for remote status.
    let exit_code = if let Some(fut) = attached.take_status() {
        match fut.await {
            Some(s) => {
                if let Some(c) = s.code {
                    c
                } else if s.status.as_deref() == Some("Success") {
                    0
                } else {
                    1
                }
            }
            None => 0,
        }
    } else {
        0
    };

    stdin_task.abort();
    stdout_task.abort();
    let _ = attached.join().await;
    // Dropping _raw restores cooked mode here.

    Ok(exit_code)
}

async fn pipe_stdin<W: AsyncWrite + Unpin>(mut w: W) {
    let mut stdin = tokio::io::stdin();
    let _ = tokio::io::copy(&mut stdin, &mut w).await;
    let _ = w.shutdown().await;
}

async fn pipe_stdout<R: AsyncRead + Unpin>(mut r: R) {
    let mut stdout = tokio::io::stdout();
    let _ = tokio::io::copy(&mut r, &mut stdout).await;
    let _ = stdout.flush().await;
}
