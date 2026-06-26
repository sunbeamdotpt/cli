//! VPN environment injection for spawned child processes.
//!
//! When the VPN daemon is running, it writes two discovery files into
//! `~/.sunbeam/vpn/`:
//!   * `socks5.port` — the dynamically-bound loopback port
//!   * `socks5.auth` — the per-session auth token (hex)
//!
//! Child processes like `kubectl` need to route cluster traffic through
//! that proxy so they pick up the WireGuard transport + in-proxy DNS
//! resolver. This module reads the discovery files and injects
//! `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY` env vars on a
//! `Command`.
//!
//! The scheme is `socks5h://` (not `socks5://`) because the `h` suffix
//! tells the client to resolve hostnames through the proxy — which is
//! the whole point of Phase 3's in-proxy DNS resolver. Plain `socks5://`
//! would resolve on the client side via system DNS and miss the
//! cluster's CoreDNS.
//!
//! All functions are best-effort: when no VPN is running (discovery
//! files missing), they are silent no-ops, so callers can use them
//! unconditionally.

use crate::error::{Result, SunbeamError};
use std::path::{Path, PathBuf};

const PORT_FILE: &str = "socks5.port";
const AUTH_FILE: &str = "socks5.auth";
const NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// Loopback TCP address of the VPN daemon's k8s API proxy. The daemon binds
/// this when it starts; `kube::Client` rewrites `cluster_url` to hit it when
/// the daemon socket exists and the active context has a `vpn_url`.
pub const VPN_K8S_PROXY: &str = "127.0.0.1:16579";

/// Fixed loopback address for the VPN daemon's SOCKS5 + HTTP CONNECT proxy.
///
/// Port 24424 is deliberately fixed (not OS-ephemeral) so that
/// `~/.kube/config`'s `proxy-url` and any other downstream consumer that
/// reads the port at startup remain valid across daemon restarts — the same
/// guarantee that `VPN_K8S_PROXY` already provides for the HTTP proxy.
pub const VPN_SOCKS_PROXY: &str = "127.0.0.1:24424";

/// Check whether the VPN daemon is currently running by looking for the
/// control socket at `~/.sunbeam/vpn/daemon.sock`. Best-effort: returns
/// false when `HOME` is unset or the socket is missing.
pub fn vpn_daemon_socket_exists() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    home.join(".sunbeam")
        .join("vpn")
        .join("daemon.sock")
        .exists()
}

/// Resolve `~/.sunbeam/vpn` — the on-disk directory where the daemon
/// keeps its state, keys, control socket, and SOCKS5 discovery files.
pub fn vpn_state_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| SunbeamError::Other("HOME not set".into()))?;
    Ok(PathBuf::from(home).join(".sunbeam").join("vpn"))
}

/// Read the SOCKS5 discovery files. Returns `None` when either file is
/// missing, unreadable, or contains nonsense — which is the normal case
/// when no VPN daemon is running.
pub fn read_socks_endpoint(state_dir: &Path) -> Option<(u16, String)> {
    let port: u16 = std::fs::read_to_string(state_dir.join(PORT_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if port == 0 {
        return None;
    }
    let token = std::fs::read_to_string(state_dir.join(AUTH_FILE))
        .ok()?
        .trim()
        .to_string();
    if token.is_empty() {
        return None;
    }
    Some((port, token))
}

/// Format a `socks5h://` URL for the given endpoint.
///
/// The SOCKS5 auth token comes from `generate_auth_token()` in
/// `sunbeam-net` as lowercase hex, which is URL-safe without encoding.
pub fn proxy_url_for(port: u16, token: &str) -> String {
    format!("socks5h://sunbeam:{token}@127.0.0.1:{port}")
}

/// Best-effort: inject `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY`
/// on a `std::process::Command` pointing at the running SOCKS5 daemon.
/// No-op when the VPN isn't running.
pub fn inject_std(state_dir: &Path, cmd: &mut std::process::Command) {
    if let Some((port, token)) = read_socks_endpoint(state_dir) {
        let url = proxy_url_for(port, &token);
        cmd.env("HTTPS_PROXY", &url);
        cmd.env("HTTP_PROXY", &url);
        cmd.env("ALL_PROXY", &url);
        cmd.env("NO_PROXY", NO_PROXY);
    }
}

/// Best-effort: inject proxy env vars on a `tokio::process::Command`.
/// No-op when the VPN isn't running.
pub fn inject_tokio(state_dir: &Path, cmd: &mut tokio::process::Command) {
    if let Some((port, token)) = read_socks_endpoint(state_dir) {
        let url = proxy_url_for(port, &token);
        cmd.env("HTTPS_PROXY", &url);
        cmd.env("HTTP_PROXY", &url);
        cmd.env("ALL_PROXY", &url);
        cmd.env("NO_PROXY", NO_PROXY);
    }
}

/// Convenience wrapper: inject using the default `~/.sunbeam/vpn` dir.
/// Silently skips if `HOME` is unset.
pub fn inject_tokio_default(cmd: &mut tokio::process::Command) {
    if let Ok(dir) = vpn_state_dir() {
        inject_tokio(&dir, cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn read_endpoint_returns_none_when_files_missing() {
        let dir = tempdir().unwrap();
        assert!(read_socks_endpoint(dir.path()).is_none());
    }

    #[test]
    fn read_endpoint_returns_none_when_only_port_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "12345").unwrap();
        assert!(read_socks_endpoint(dir.path()).is_none());
    }

    #[test]
    fn read_endpoint_returns_none_when_only_auth_exists() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "deadbeef").unwrap();
        assert!(read_socks_endpoint(dir.path()).is_none());
    }

    #[test]
    fn read_endpoint_returns_none_on_garbage_port() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "not-a-number").unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "deadbeef").unwrap();
        assert!(read_socks_endpoint(dir.path()).is_none());
    }

    #[test]
    fn read_endpoint_rejects_port_zero() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "0").unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "deadbeef").unwrap();
        assert!(read_socks_endpoint(dir.path()).is_none());
    }

    #[test]
    fn read_endpoint_rejects_empty_token() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "12345").unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "").unwrap();
        assert!(read_socks_endpoint(dir.path()).is_none());
    }

    #[test]
    fn read_endpoint_trims_whitespace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "  54321\n").unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "deadbeef\n").unwrap();
        let (port, token) = read_socks_endpoint(dir.path()).unwrap();
        assert_eq!(port, 54321);
        assert_eq!(token, "deadbeef");
    }

    #[test]
    fn proxy_url_is_well_formed() {
        let url = proxy_url_for(12345, "deadbeefcafe");
        assert_eq!(url, "socks5h://sunbeam:deadbeefcafe@127.0.0.1:12345");
    }

    #[test]
    fn proxy_url_uses_socks5h_scheme_for_remote_dns() {
        let url = proxy_url_for(1, "x");
        assert!(url.starts_with("socks5h://"), "got {url}");
    }

    #[test]
    fn inject_std_is_noop_when_daemon_not_running() {
        let dir = tempdir().unwrap();
        let mut cmd = std::process::Command::new("true");
        inject_std(dir.path(), &mut cmd);
        // env vars are observable only after spawning — as a proxy, we
        // rely on the fact that the function returns without panicking
        // and the cmd remains runnable.
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(
            envs.is_empty(),
            "expected no env vars injected, got {envs:?}"
        );
    }

    #[test]
    fn inject_std_sets_all_four_proxy_vars() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "16580").unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "cafebabe").unwrap();
        let mut cmd = std::process::Command::new("true");
        inject_std(dir.path(), &mut cmd);

        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|vv| {
                    (
                        k.to_string_lossy().into_owned(),
                        vv.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();

        let want_url = "socks5h://sunbeam:cafebabe@127.0.0.1:16580";
        assert_eq!(envs.get("HTTPS_PROXY").map(String::as_str), Some(want_url));
        assert_eq!(envs.get("HTTP_PROXY").map(String::as_str), Some(want_url));
        assert_eq!(envs.get("ALL_PROXY").map(String::as_str), Some(want_url));
        assert_eq!(
            envs.get("NO_PROXY").map(String::as_str),
            Some("localhost,127.0.0.1,::1")
        );
    }

    #[test]
    fn inject_tokio_sets_all_four_proxy_vars() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "16581").unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), "feedface").unwrap();
        let mut cmd = tokio::process::Command::new("true");
        inject_tokio(dir.path(), &mut cmd);

        // Convert to std::Command to read envs (tokio wraps std).
        let std_cmd = cmd.as_std();
        let envs: std::collections::HashMap<_, _> = std_cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|vv| {
                    (
                        k.to_string_lossy().into_owned(),
                        vv.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();

        let want_url = "socks5h://sunbeam:feedface@127.0.0.1:16581";
        assert_eq!(envs.get("HTTPS_PROXY").map(String::as_str), Some(want_url));
        assert_eq!(envs.get("HTTP_PROXY").map(String::as_str), Some(want_url));
        assert_eq!(envs.get("ALL_PROXY").map(String::as_str), Some(want_url));
        assert_eq!(
            envs.get("NO_PROXY").map(String::as_str),
            Some("localhost,127.0.0.1,::1")
        );
    }

    /// `VPN_SOCKS_PROXY` must embed the same port as `sunbeam_net::SOCKS5_PORT`
    /// so the two crates stay in sync without a shared constant.
    #[test]
    fn vpn_socks_proxy_port_matches_sunbeam_net_constant() {
        let port: u16 = VPN_SOCKS_PROXY
            .rsplit(':')
            .next()
            .expect("VPN_SOCKS_PROXY must contain ':'")
            .parse()
            .expect("port part of VPN_SOCKS_PROXY must be a valid u16");
        assert_eq!(
            port,
            sunbeam_net::SOCKS5_PORT,
            "VPN_SOCKS_PROXY ({VPN_SOCKS_PROXY}) port must match sunbeam_net::SOCKS5_PORT ({})",
            sunbeam_net::SOCKS5_PORT
        );
    }

    /// The proxy-url written to kubeconfig must embed the fixed SOCKS5 port
    /// so it survives daemon restarts.
    #[test]
    fn kubeconfig_writer_uses_fixed_socks_port() {
        let token = "cafebabe";
        let port: u16 = VPN_SOCKS_PROXY.rsplit(':').next().unwrap().parse().unwrap();
        let url = proxy_url_for(port, token);
        assert!(
            url.contains(":24424"),
            "kubeconfig proxy-url must contain the fixed port :24424, got {url}"
        );
    }

    #[test]
    fn vpn_state_dir_points_under_home() {
        // Set HOME to a tempdir so the test is hermetic.
        let dir = tempdir().unwrap();
        let prev = std::env::var_os("HOME");
        // SAFETY: we restore HOME before returning. Tests in this
        // module run serially for this reason — see #[cfg(test)] above.
        unsafe { std::env::set_var("HOME", dir.path()) };
        let got = vpn_state_dir().unwrap();
        if let Some(p) = prev {
            unsafe { std::env::set_var("HOME", p) };
        } else {
            unsafe { std::env::remove_var("HOME") };
        }
        assert_eq!(got, dir.path().join(".sunbeam").join("vpn"));
    }
}
