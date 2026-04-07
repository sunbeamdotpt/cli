//! `sunbeam connect` / `sunbeam disconnect` / `sunbeam vpn ...`
//!
//! `sunbeam connect` re-execs the current binary with a hidden
//! `__vpn-daemon` subcommand and detaches it (stdio → /dev/null + a log
//! file). The detached child runs the actual `sunbeam-net` daemon and
//! listens on the IPC control socket. The user-facing process polls the
//! socket until the daemon reaches Running, prints status, and exits.
//!
//! This shape avoids forking from inside the tokio runtime.

use crate::config::active_context;
use crate::error::{Result, SunbeamError};
use crate::output::{ok, step, warn};
use std::path::PathBuf;

/// Run `sunbeam connect`.
///
/// Default mode spawns a backgrounded daemon and returns once it reaches
/// Running. With `--foreground`, runs the daemon in-process and blocks
/// until SIGINT or SIGTERM.
pub async fn cmd_connect(foreground: bool) -> Result<()> {
    let ctx = active_context();
    if ctx.vpn_url.is_empty() {
        return Err(SunbeamError::Other(
            "no VPN configured for this context — set vpn-url and vpn-auth-key in config".into(),
        ));
    }
    if ctx.vpn_auth_key.is_empty() {
        return Err(SunbeamError::Other(
            "no VPN auth key for this context — set vpn-auth-key in config".into(),
        ));
    }

    let state_dir = vpn_state_dir()?;
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        SunbeamError::Other(format!("create vpn state dir {}: {e}", state_dir.display()))
    })?;

    if foreground {
        return run_daemon_foreground().await;
    }

    spawn_background_daemon(&state_dir).await
}

/// Spawn a detached daemon child and wait for it to reach Running.
async fn spawn_background_daemon(state_dir: &std::path::Path) -> Result<()> {
    // Refuse to start a second daemon if one is already running.
    let socket = state_dir.join("daemon.sock");
    let probe = sunbeam_net::IpcClient::new(&socket);
    if probe.socket_exists() {
        if let Ok(status) = probe.status().await {
            warn(&format!(
                "VPN daemon already running ({status}). Use `sunbeam disconnect` first."
            ));
            return Ok(());
        }
        // Stale socket — clean it up so the new daemon can rebind.
        let _ = std::fs::remove_file(&socket);
    }

    // Re-exec ourselves with the hidden subcommand.
    let exe = std::env::current_exe()
        .map_err(|e| SunbeamError::Other(format!("locate current_exe: {e}")))?;
    let log_path = state_dir.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| SunbeamError::Other(format!("open daemon log: {e}")))?;
    let log_err = log
        .try_clone()
        .map_err(|e| SunbeamError::Other(format!("dup daemon log fd: {e}")))?;

    let mut cmd = std::process::Command::new(&exe);
    // --context is a top-level flag, must precede the subcommand.
    let cfg = crate::config::load_config();
    if !cfg.current_context.is_empty() {
        cmd.arg("--context").arg(&cfg.current_context);
    }
    cmd.arg("__vpn-daemon");
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log_err));

    // Detach from the controlling terminal so closing the parent shell
    // doesn't SIGHUP the daemon.
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            // Become a session leader so the child has no controlling TTY.
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .map_err(|e| SunbeamError::Other(format!("spawn daemon: {e}")))?;

    step(&format!(
        "VPN daemon spawned (pid {}, logs at {})",
        child.id(),
        log_path.display()
    ));

    // Poll the IPC socket until the daemon reaches Running.
    let client = sunbeam_net::IpcClient::new(&socket);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if std::time::Instant::now() > deadline {
            warn(
                "VPN daemon did not reach Running state within 30s — \
                 check the daemon log for details",
            );
            return Ok(());
        }
        if !client.socket_exists() {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            continue;
        }
        match client.status().await {
            Ok(sunbeam_net::DaemonStatus::Running {
                addresses,
                peer_count,
                ..
            }) => {
                let addrs: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
                ok(&format!(
                    "Connected ({}) — {} peers visible",
                    addrs.join(", "),
                    peer_count
                ));
                return Ok(());
            }
            Ok(sunbeam_net::DaemonStatus::Error { message }) => {
                return Err(SunbeamError::Other(format!("VPN daemon error: {message}")));
            }
            // Still starting / connecting / registering — keep polling.
            Ok(_) | Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
    }
}

/// The hidden `__vpn-daemon` subcommand entry point.
pub async fn cmd_vpn_daemon() -> Result<()> {
    run_daemon_foreground().await
}

/// Build VpnConfig from the active context, start the daemon, and block
/// until SIGINT/SIGTERM or an IPC `Stop` request brings it down.
async fn run_daemon_foreground() -> Result<()> {
    let ctx = active_context();
    let state_dir = vpn_state_dir()?;
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        SunbeamError::Other(format!("create vpn state dir {}: {e}", state_dir.display()))
    })?;

    let user = whoami::username().unwrap_or_else(|_| "unknown".to_string());
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());
    let hostname = format!("{user}@{host}");

    let config = sunbeam_net::VpnConfig {
        coordination_url: ctx.vpn_url.clone(),
        auth_key: ctx.vpn_auth_key.clone(),
        state_dir: state_dir.clone(),
        // Bind the local k8s proxy on 16579 — far enough away from common
        // conflicts (6443 = kube API, 16443 = sienna's SSH tunnel) that we
        // shouldn't collide on dev machines. TODO: make this configurable.
        proxy_bind: "127.0.0.1:16579".parse().expect("static addr"),
        // Static fallback if the netmap doesn't have the named host.
        cluster_api_addr: "100.64.0.1".parse().expect("static addr"),
        cluster_api_port: 6443,
        // If the user set vpn-cluster-host in their context config, the
        // daemon resolves it from the netmap and uses that peer's
        // tailnet IP for the proxy backend.
        cluster_api_host: if ctx.vpn_cluster_host.is_empty() {
            None
        } else {
            Some(ctx.vpn_cluster_host.clone())
        },
        control_socket: state_dir.join("daemon.sock"),
        hostname,
        server_public_key: None,
    };

    step(&format!("Connecting to {}", ctx.vpn_url));
    let handle = sunbeam_net::VpnDaemon::start(config)
        .await
        .map_err(|e| SunbeamError::Other(format!("daemon start: {e}")))?;

    // Wait for either Ctrl-C, SIGTERM, or the daemon stopping itself
    // (e.g. via an IPC `Stop` request).
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| SunbeamError::Other(format!("install SIGTERM handler: {e}")))?;

    loop {
        tokio::select! {
            biased;
            _ = &mut ctrl_c => {
                step("Interrupt — disconnecting...");
                break;
            }
            _ = sigterm.recv() => {
                step("SIGTERM — disconnecting...");
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if matches!(handle.current_status(), sunbeam_net::DaemonStatus::Stopped) {
                    break;
                }
            }
        }
    }

    handle
        .shutdown()
        .await
        .map_err(|e| SunbeamError::Other(format!("daemon shutdown: {e}")))?;
    ok("Disconnected.");
    Ok(())
}

/// Run `sunbeam disconnect` — signal a running daemon via its IPC socket.
pub async fn cmd_disconnect() -> Result<()> {
    let socket = vpn_state_dir()?.join("daemon.sock");
    let client = sunbeam_net::IpcClient::new(&socket);
    if !client.socket_exists() {
        return Err(SunbeamError::Other(
            "no running VPN daemon (control socket missing)".into(),
        ));
    }
    step("Asking VPN daemon to stop...");
    client
        .stop()
        .await
        .map_err(|e| SunbeamError::Other(format!("IPC stop: {e}")))?;
    ok("Daemon acknowledged shutdown.");
    Ok(())
}

/// Run `sunbeam vpn status` — query a running daemon's status via IPC.
pub async fn cmd_vpn_status() -> Result<()> {
    let socket = vpn_state_dir()?.join("daemon.sock");
    let client = sunbeam_net::IpcClient::new(&socket);
    if !client.socket_exists() {
        println!("VPN: not running");
        return Ok(());
    }
    match client.status().await {
        Ok(sunbeam_net::DaemonStatus::Running { addresses, peer_count, derp_home }) => {
            let addrs: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
            println!("VPN: running");
            println!("  addresses: {}", addrs.join(", "));
            println!("  peers: {peer_count}");
            if let Some(region) = derp_home {
                println!("  derp home: region {region}");
            }
        }
        Ok(other) => {
            println!("VPN: {other}");
        }
        Err(e) => {
            // Socket exists but daemon isn't actually responding — common
            // when the daemon crashed and left a stale socket file behind.
            println!("VPN: stale socket at {} ({e})", socket.display());
        }
    }
    Ok(())
}

/// Resolve the on-disk directory for VPN state (keys, control socket).
fn vpn_state_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| SunbeamError::Other("HOME not set".into()))?;
    Ok(PathBuf::from(home).join(".sunbeam").join("vpn"))
}
