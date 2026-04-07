//! `sunbeam connect` / `sunbeam disconnect` / `sunbeam vpn ...`
//!
//! These commands wrap the `sunbeam-net` daemon and run it in the foreground
//! of the CLI process. We don't currently background it as a separate
//! daemon process — running in the foreground keeps the lifecycle simple
//! and is the right shape for the typical workflow ("connect, do work,
//! disconnect with ^C").

use crate::config::active_context;
use crate::error::{Result, SunbeamError};
use crate::output::{ok, step, warn};
use std::path::PathBuf;

/// Run `sunbeam connect` — start the VPN daemon and block until shutdown.
pub async fn cmd_connect() -> Result<()> {
    let ctx = active_context();

    if ctx.vpn_url.is_empty() {
        return Err(SunbeamError::Other(
            "no VPN configured for this context — set vpn-url and vpn-auth-key in config"
                .into(),
        ));
    }
    if ctx.vpn_auth_key.is_empty() {
        return Err(SunbeamError::Other(
            "no VPN auth key for this context — set vpn-auth-key in config".into(),
        ));
    }

    let state_dir = vpn_state_dir()?;
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        SunbeamError::Other(format!(
            "create vpn state dir {}: {e}",
            state_dir.display()
        ))
    })?;

    // Build the netmap label as "<user>@<host>" so multiple workstations
    // for the same human are distinguishable in `headscale nodes list`.
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
        // Bind the local k8s proxy on a fixed port the rest of the CLI can
        // discover via context (or via IPC, eventually).
        proxy_bind: "127.0.0.1:16443".parse().expect("static addr"),
        // Default cluster API target — TODO: derive from netmap once we
        // know which peer hosts the k8s API.
        cluster_api_addr: "100.64.0.1".parse().expect("static addr"),
        cluster_api_port: 6443,
        control_socket: state_dir.join("daemon.sock"),
        hostname,
        server_public_key: None,
    };

    step(&format!("Connecting to {}", ctx.vpn_url));
    let handle = sunbeam_net::VpnDaemon::start(config)
        .await
        .map_err(|e| SunbeamError::Other(format!("daemon start: {e}")))?;

    // Block until the daemon reaches Running, then sit on it until SIGINT.
    let mut ready = false;
    for _ in 0..60 {
        match handle.current_status() {
            sunbeam_net::DaemonStatus::Running { addresses, peer_count, .. } => {
                let addrs: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
                ok(&format!(
                    "Connected ({}) — {} peers visible",
                    addrs.join(", "),
                    peer_count
                ));
                ready = true;
                break;
            }
            sunbeam_net::DaemonStatus::Reconnecting { attempt } => {
                warn(&format!("Reconnecting (attempt {attempt})..."));
            }
            sunbeam_net::DaemonStatus::Error { ref message } => {
                return Err(SunbeamError::Other(format!("VPN error: {message}")));
            }
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    if !ready {
        warn("VPN daemon did not reach Running state within 30s — continuing anyway");
    }

    println!("Press Ctrl-C to disconnect.");
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| SunbeamError::Other(format!("install signal handler: {e}")))?;
    step("Disconnecting...");
    handle
        .shutdown()
        .await
        .map_err(|e| SunbeamError::Other(format!("daemon shutdown: {e}")))?;
    ok("Disconnected.");
    Ok(())
}

/// Run `sunbeam disconnect` — signal a running daemon via its IPC socket.
pub async fn cmd_disconnect() -> Result<()> {
    let state_dir = vpn_state_dir()?;
    let socket = state_dir.join("daemon.sock");
    if !socket.exists() {
        return Err(SunbeamError::Other(
            "no running VPN daemon (control socket missing)".into(),
        ));
    }
    // The daemon's IPC server lives in sunbeam_net::daemon::ipc, but it's
    // not currently exported as a client. Until that lands, the canonical
    // way to disconnect is to ^C the foreground `sunbeam connect` process.
    Err(SunbeamError::Other(
        "background daemon control not yet implemented — Ctrl-C the running `sunbeam connect`"
            .into(),
    ))
}

/// Run `sunbeam vpn status` — query a running daemon's status via IPC.
pub async fn cmd_vpn_status() -> Result<()> {
    let state_dir = vpn_state_dir()?;
    let socket = state_dir.join("daemon.sock");
    if !socket.exists() {
        println!("VPN: not running");
        return Ok(());
    }
    println!("VPN: running (control socket at {})", socket.display());
    // TODO: actually query the IPC socket once the IPC client API is
    // exposed from sunbeam-net.
    Ok(())
}

/// Resolve the on-disk directory for VPN state (keys, control socket).
fn vpn_state_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| SunbeamError::Other("HOME not set".into()))?;
    Ok(PathBuf::from(home).join(".sunbeam").join("vpn"))
}
