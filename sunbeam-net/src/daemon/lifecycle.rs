use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use smoltcp::wire::IpAddress;
use tokio::sync::{mpsc, oneshot};

use crate::config::VpnConfig;
use crate::control::MapUpdate;
use crate::daemon::ipc::IpcServer;
use crate::daemon::state::{DaemonHandle, DaemonStatus};
use crate::derp::client::DerpClient;
use crate::proto::types::DerpMap;
use crate::proxy::engine::{EngineCommand, NetworkEngine};
use crate::wg::tunnel::{DecapAction, WgTunnel};

/// The main VPN daemon that coordinates all subsystems.
pub struct VpnDaemon;

impl VpnDaemon {
    /// Start the VPN daemon in a background task.
    /// Returns a handle for status queries and shutdown.
    pub async fn start(config: VpnConfig) -> crate::Result<DaemonHandle> {
        let status = Arc::new(RwLock::new(DaemonStatus::Connecting));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let status_clone = status.clone();
        let join = tokio::spawn(async move {
            run_daemon_loop(config, status_clone, shutdown_rx).await
        });

        Ok(DaemonHandle::with_daemon(shutdown_tx, status, join))
    }
}

/// Main daemon loop with reconnection.
async fn run_daemon_loop(
    config: VpnConfig,
    status: Arc<RwLock<DaemonStatus>>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> crate::Result<()> {
    let keys = crate::keys::NodeKeys::load_or_generate(&config.state_dir)?;
    let mut attempt: u32 = 0;
    let max_backoff = Duration::from_secs(60);

    loop {
        set_status(&status, DaemonStatus::Connecting);

        let session = run_session(&config, &keys, &status);
        tokio::pin!(session);

        let session_result = tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                set_status(&status, DaemonStatus::Stopped);
                return Ok(());
            }
            r = &mut session => r,
        };

        match session_result {
            Ok(SessionExit::Disconnected) | Err(_) => {
                attempt += 1;
                let delay = std::cmp::min(
                    Duration::from_secs(1 << attempt.min(6)),
                    max_backoff,
                );
                set_status(&status, DaemonStatus::Reconnecting { attempt });

                tokio::select! {
                    _ = tokio::time::sleep(delay) => continue,
                    _ = &mut shutdown_rx => {
                        set_status(&status, DaemonStatus::Stopped);
                        return Ok(());
                    }
                }
            }
        }
    }
}

enum SessionExit {
    Disconnected,
}

/// Run a single VPN session. Returns when the session ends (error or shutdown).
async fn run_session(
    config: &VpnConfig,
    keys: &crate::keys::NodeKeys,
    status: &Arc<RwLock<DaemonStatus>>,
) -> std::result::Result<SessionExit, crate::Error> {
    // 1. Connect to coordination server
    set_status(status, DaemonStatus::Connecting);
    let mut control = crate::control::ControlClient::connect(config, keys).await
        .map_err(|e| { eprintln!("[session] connect: {e:?}"); e })?;

    // 2. Register
    set_status(status, DaemonStatus::Registering);
    let _reg = control.register(&config.auth_key, &config.hostname, keys).await?;

    // 3. Start map stream
    let mut map_stream = control.map_stream(keys, &config.hostname).await?;

    // 4. Wait for first netmap to get our addresses and peers
    let first_update = map_stream.next().await?
        .ok_or_else(|| crate::Error::Control("map stream closed before first update".into()))?;

    let (peers, addresses, derp_map) = match &first_update {
        MapUpdate::Full { peers, self_node, derp_map, .. } => {
            let addrs: Vec<IpAddr> = self_node.addresses.iter()
                .filter_map(|a| a.split('/').next()?.parse().ok())
                .collect();
            (peers.clone(), addrs, derp_map.clone())
        }
        _ => {
            return Err(crate::Error::Control("expected Full netmap as first update".into()));
        }
    };

    let peer_count = peers.len();

    // 5. Initialize WireGuard tunnel with our WG private key
    let mut wg_tunnel = WgTunnel::new(keys.wg_private.clone());
    wg_tunnel.update_peers(&peers);

    // 6. Set up NetworkEngine with our VPN IP
    let local_ip = addresses.first()
        .ok_or_else(|| crate::Error::Control("no addresses assigned".into()))?;
    let smoltcp_ip = match local_ip {
        IpAddr::V4(v4) => IpAddress::Ipv4(smoltcp::wire::Ipv4Address::from(*v4)),
        IpAddr::V6(v6) => IpAddress::Ipv6(smoltcp::wire::Ipv6Address::from(*v6)),
    };

    let (engine, channels) = NetworkEngine::new(smoltcp_ip, 10)?;

    // 7. Start TCP proxy that routes through the engine
    let cancel = tokio_util::sync::CancellationToken::new();
    let proxy_cmd_tx = channels.cmd_tx.clone();
    let proxy_bind = config.proxy_bind;
    let cluster_addr = std::net::SocketAddr::new(config.cluster_api_addr, config.cluster_api_port);

    // Proxy listener task: accepts local connections and sends them to the engine
    let proxy_cancel = cancel.clone();
    let proxy_task = tokio::spawn(async move {
        run_proxy_listener(proxy_bind, cluster_addr, proxy_cmd_tx, proxy_cancel).await
    });

    // 8. Engine task: polls smoltcp and bridges connections
    let engine_cancel = cancel.clone();
    let engine_task = tokio::spawn(async move {
        engine.run(engine_cancel).await;
    });

    // 9. Connect to DERP relay (for now: pick first node from derp_map)
    let (derp_out_tx, derp_out_rx) = mpsc::channel::<([u8; 32], Vec<u8>)>(256);
    let (derp_in_tx, derp_in_rx) = mpsc::channel::<([u8; 32], Vec<u8>)>(256);

    let derp_endpoint = derp_map
        .as_ref()
        .and_then(pick_derp_node)
        .filter(|(host, port)| !host.is_empty() && *port != 0)
        .or_else(|| coordination_host_port(&config.coordination_url));

    let _derp_task = if let Some((host, port)) = derp_endpoint {
        let url = format!("{host}:{port}");
        tracing::info!("connecting to DERP relay at {url}");
        match DerpClient::connect(&url, keys).await {
            Ok(client) => {
                tracing::info!("DERP relay connected: {url}");
                let derp_cancel = cancel.clone();
                Some(tokio::spawn(async move {
                    run_derp_loop(client, derp_out_rx, derp_in_tx, derp_cancel).await;
                }))
            }
            Err(e) => {
                tracing::warn!("DERP connect failed: {e}; continuing without relay");
                None
            }
        }
    } else {
        tracing::warn!("no DERP endpoint available; continuing without relay");
        None
    };

    // 10. WG encap/decap task: bridges engine IP packets ↔ WG ↔ transport
    let engine_to_wg_rx = channels.engine_to_wg_rx;
    let wg_to_engine_tx = channels.wg_to_engine_tx;
    let wg_cancel = cancel.clone();
    let wg_task = tokio::spawn(async move {
        run_wg_loop(
            wg_tunnel,
            engine_to_wg_rx,
            wg_to_engine_tx,
            derp_out_tx,
            derp_in_rx,
            wg_cancel,
        )
        .await
    });

    // 11. Start IPC server
    let ipc = IpcServer::new(&config.control_socket, status.clone())?;

    // Mark as ready
    let derp_home = derp_map.as_ref()
        .and_then(|dm| dm.regions.values().next())
        .map(|r| r.region_id);

    set_status(status, DaemonStatus::Running {
        addresses,
        peer_count,
        derp_home,
    });

    // 11. Run concurrent tasks
    tokio::select! {
        result = map_stream_loop(&mut map_stream, status) => {
            eprintln!("[session] map_stream_loop exited: {result:?}");
            cancel.cancel();
            match result {
                Ok(()) => Ok(SessionExit::Disconnected),
                Err(e) => Err(e),
            }
        }
        r = proxy_task => {
            eprintln!("[session] proxy_task exited: {r:?}");
            cancel.cancel();
            Ok(SessionExit::Disconnected)
        }
        r = engine_task => {
            eprintln!("[session] engine_task exited: {r:?}");
            cancel.cancel();
            Ok(SessionExit::Disconnected)
        }
        r = wg_task => {
            eprintln!("[session] wg_task exited: {r:?}");
            cancel.cancel();
            Ok(SessionExit::Disconnected)
        }
        result = ipc.run() => {
            eprintln!("[session] ipc.run() exited: {result:?}");
            cancel.cancel();
            result.map(|_| SessionExit::Disconnected)
        }
    }
}

/// Accept local TCP connections and forward them to the engine.
async fn run_proxy_listener(
    bind_addr: std::net::SocketAddr,
    remote_addr: std::net::SocketAddr,
    cmd_tx: mpsc::Sender<EngineCommand>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind_addr).await
        .map_err(|e| crate::Error::Io {
            context: format!("bind proxy {bind_addr}"),
            source: e,
        })?;

    tracing::info!("VPN proxy listening on {}", listener.local_addr().unwrap_or(bind_addr));

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, peer) = accept.map_err(|e| crate::Error::Io {
                    context: "accept proxy".into(),
                    source: e,
                })?;
                tracing::debug!("proxy connection from {peer} → {remote_addr}");
                let _ = cmd_tx.send(EngineCommand::NewConnection {
                    local: stream,
                    remote: remote_addr,
                }).await;
            }
            _ = cancel.cancelled() => {
                tracing::info!("proxy listener shutting down");
                return Ok(());
            }
        }
    }
}

/// WireGuard encapsulation/decapsulation loop.
///
/// Reads IP packets from the engine, encapsulates them through WireGuard,
/// and sends WG packets out via DERP. Receives WG packets from DERP,
/// decapsulates, and feeds IP packets back to the engine.
async fn run_wg_loop(
    mut tunnel: WgTunnel,
    mut from_engine: mpsc::Receiver<Vec<u8>>,
    to_engine: mpsc::Sender<Vec<u8>>,
    derp_out_tx: mpsc::Sender<([u8; 32], Vec<u8>)>,
    mut derp_in_rx: mpsc::Receiver<([u8; 32], Vec<u8>)>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut tick_interval = tokio::time::interval(Duration::from_millis(250));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // TODO: wire UDP socket for direct transport (currently DERP-only)

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            ip_packet = from_engine.recv() => {
                match ip_packet {
                    Some(packet) => {
                        if let Some(dst_ip) = parse_dst_ip(&packet) {
                            let action = tunnel.encapsulate(dst_ip, &packet);
                            dispatch_encap(action, &derp_out_tx).await;
                        }
                    }
                    None => return, // engine dropped
                }
            }
            incoming = derp_in_rx.recv() => {
                match incoming {
                    Some((src_key, data)) => {
                        let action = tunnel.decapsulate(&src_key, &data);
                        match action {
                            DecapAction::Packet(p) => {
                                if to_engine.send(p).await.is_err() {
                                    return;
                                }
                            }
                            DecapAction::Response(r) => {
                                let _ = derp_out_tx.send((src_key, r)).await;
                            }
                            DecapAction::Nothing => {}
                        }
                    }
                    None => return, // DERP loop dropped
                }
            }
            _ = tick_interval.tick() => {
                let actions = tunnel.tick();
                for ta in actions {
                    dispatch_encap(ta.action, &derp_out_tx).await;
                }
            }
        }
    }
}

/// Dispatch a WG encap action to the appropriate transport.
async fn dispatch_encap(
    action: crate::wg::tunnel::EncapAction,
    derp_out_tx: &mpsc::Sender<([u8; 32], Vec<u8>)>,
) {
    match action {
        crate::wg::tunnel::EncapAction::SendUdp { endpoint, data } => {
            tracing::trace!("WG → UDP {endpoint} ({} bytes) — UDP transport not implemented", data.len());
            // TODO: send via UDP socket
        }
        crate::wg::tunnel::EncapAction::SendDerp { dest_key, data } => {
            tracing::trace!("WG → DERP ({} bytes)", data.len());
            let _ = derp_out_tx.send((dest_key, data)).await;
        }
        crate::wg::tunnel::EncapAction::Nothing => {}
    }
}

/// DERP relay loop: bridges packets between WG layer and a DERP client.
async fn run_derp_loop(
    mut client: DerpClient,
    mut out_rx: mpsc::Receiver<([u8; 32], Vec<u8>)>,
    in_tx: mpsc::Sender<([u8; 32], Vec<u8>)>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::debug!("DERP loop shutting down");
                return;
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some((dest_key, data)) => {
                        if let Err(e) = client.send_packet(&dest_key, &data).await {
                            tracing::warn!("DERP send failed: {e}");
                            return;
                        }
                    }
                    None => return,
                }
            }
            incoming = client.recv_packet() => {
                match incoming {
                    Ok((src_key, data)) => {
                        if in_tx.send((src_key, data)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("DERP recv failed: {e}");
                        return;
                    }
                }
            }
        }
    }
}

/// Pick the first DERP node from the map (any region, any node).
fn pick_derp_node(derp_map: &DerpMap) -> Option<(String, u16)> {
    derp_map
        .regions
        .values()
        .flat_map(|r| r.nodes.iter())
        .next()
        .map(|n| (n.host_name.clone(), n.derp_port))
}

/// Extract host:port from a coordination URL like `http://localhost:8080`.
fn coordination_host_port(url: &str) -> Option<(String, u16)> {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let path_end = stripped.find('/').unwrap_or(stripped.len());
    let authority = &stripped[..path_end];
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => {
            // Default port based on scheme.
            let default = if url.starts_with("https://") { 443 } else { 80 };
            (authority.to_string(), default)
        }
    };
    Some((host, port))
}

/// Continuously read map updates and update state.
async fn map_stream_loop(
    stream: &mut crate::control::MapStream,
    status: &Arc<RwLock<DaemonStatus>>,
) -> crate::Result<()> {
    loop {
        match stream.next().await? {
            Some(update) => {
                match &update {
                    MapUpdate::Full { peers, .. } => {
                        update_peer_count(status, peers.len());
                        tracing::info!("netmap: {} peers", peers.len());
                    }
                    MapUpdate::PeersChanged(peers) => {
                        tracing::debug!("netmap: {} peers changed", peers.len());
                    }
                    MapUpdate::PeersRemoved(keys) => {
                        tracing::debug!("netmap: {} peers removed", keys.len());
                    }
                    MapUpdate::KeepAlive => {
                        tracing::trace!("netmap: keep-alive");
                    }
                }
            }
            None => return Ok(()), // stream ended
        }
    }
}

/// Parse the destination IP address from a raw IP packet.
fn parse_dst_ip(packet: &[u8]) -> Option<IpAddr> {
    if packet.is_empty() {
        return None;
    }
    let version = packet[0] >> 4;
    match version {
        4 if packet.len() >= 20 => {
            let dst = std::net::Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
            Some(IpAddr::V4(dst))
        }
        6 if packet.len() >= 40 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&packet[24..40]);
            Some(IpAddr::V6(std::net::Ipv6Addr::from(octets)))
        }
        _ => None,
    }
}

fn set_status(status: &Arc<RwLock<DaemonStatus>>, new: DaemonStatus) {
    if let Ok(mut s) = status.write() {
        *s = new;
    }
}

fn update_peer_count(status: &Arc<RwLock<DaemonStatus>>, count: usize) {
    if let Ok(mut s) = status.write() {
        if let DaemonStatus::Running { peer_count, .. } = &mut *s {
            *peer_count = count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_daemon_start_without_server() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = VpnConfig {
            coordination_url: "http://127.0.0.1:1".to_string(), // bogus, won't connect
            auth_key: "test-key".to_string(),
            state_dir: dir.path().to_path_buf(),
            proxy_bind: "127.0.0.1:0".parse().unwrap(),
            cluster_api_addr: "10.0.0.1".parse().unwrap(),
            cluster_api_port: 6443,
            control_socket: dir.path().join("test.sock"),
            hostname: "test-node".to_string(),
            server_public_key: Some([0xaa; 32]),
        };

        let handle = VpnDaemon::start(config).await.unwrap();

        // Give the daemon a moment to attempt connection and enter reconnecting state.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let status = handle.current_status();
        // It should be reconnecting (connection to bogus addr fails) or connecting.
        match status {
            DaemonStatus::Reconnecting { .. }
            | DaemonStatus::Connecting => {}
            other => panic!("expected Reconnecting or Connecting, got {other:?}"),
        }

        handle.shutdown().await.unwrap();
    }
}
