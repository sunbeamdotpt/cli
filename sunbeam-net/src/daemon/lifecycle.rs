use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use smoltcp::wire::IpAddress;
use tokio::sync::mpsc;

use crate::config::VpnConfig;
use crate::control::{MapUpdate, RouteTable};
use crate::daemon::ipc::IpcServer;
use crate::daemon::state::{DaemonHandle, DaemonStatus};
use crate::derp::client::DerpTlsMode;
use crate::derp::manager::DerpManager;
use crate::disco::packet::PacketKind;
use crate::proto::types::Node;
use crate::proxy::audit::AuditLog;
use crate::proxy::engine::{EngineCommand, NetworkEngine};
use crate::wg::tunnel::{DecapAction, WgTunnel};

/// The main VPN daemon that coordinates all subsystems.
pub struct VpnDaemon;

impl VpnDaemon {
    /// Start the VPN daemon in a background task.
    /// Returns a handle for status queries and shutdown.
    pub async fn start(config: VpnConfig) -> crate::Result<DaemonHandle> {
        let status = Arc::new(RwLock::new(DaemonStatus::Connecting));
        // CancellationToken (rather than oneshot) so both DaemonHandle::shutdown
        // and the IPC server can trigger it.
        let shutdown = tokio_util::sync::CancellationToken::new();

        let status_clone = status.clone();
        let shutdown_clone = shutdown.clone();
        let join =
            tokio::spawn(
                async move { run_daemon_loop(config, status_clone, shutdown_clone).await },
            );

        Ok(DaemonHandle::with_daemon(shutdown, status, join))
    }
}

/// Main daemon loop with reconnection.
async fn run_daemon_loop(
    config: VpnConfig,
    status: Arc<RwLock<DaemonStatus>>,
    shutdown: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    // Make sure the IPC control socket is cleaned up no matter how the
    // daemon exits — otherwise `sunbeam vpn status` after a clean shutdown
    // would see a stale socket file and report "stale socket".
    let _socket_guard = SocketGuard::new(config.control_socket.clone());

    let mut attempt: u32 = 0;
    let max_backoff = Duration::from_secs(60);

    loop {
        // Reload keys every iteration so a key rotation written in the previous
        // session is picked up before re-registering.
        let keys = crate::keys::NodeKeys::load_or_generate(&config.state_dir)?;

        set_status(&status, DaemonStatus::Connecting);

        let session = run_session(&config, &keys, &status, &shutdown);
        tokio::pin!(session);

        let session_result = tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                set_status(&status, DaemonStatus::Stopped);
                return Ok(());
            }
            r = &mut session => r,
        };

        match session_result {
            Ok(SessionExit::RotatedKey) => {
                // Keys already written; reload happens at the top of the next
                // iteration. Skip backoff — we want to re-register quickly.
                attempt = 0;
                continue;
            }
            Ok(SessionExit::Disconnected) | Err(_) => {
                if let Err(ref e) = session_result {
                    tracing::error!("session ended with error: {e:?}");
                }
                attempt += 1;
                let delay = std::cmp::min(Duration::from_secs(1 << attempt.min(6)), max_backoff);
                set_status(&status, DaemonStatus::Reconnecting { attempt });

                tokio::select! {
                    _ = tokio::time::sleep(delay) => continue,
                    _ = shutdown.cancelled() => {
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
    /// The stuck-handshake watchdog rotated the node key. The outer loop must
    /// reload keys before starting the next session.
    RotatedKey,
}

/// RAII guard that removes a Unix socket file when dropped. Used by
/// `run_daemon_loop` to make sure the IPC control socket is cleaned up
/// when the daemon exits, regardless of whether shutdown was triggered
/// via DaemonHandle, IPC Stop, signal, or panic.
struct SocketGuard {
    path: std::path::PathBuf,
}

impl SocketGuard {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Cleans up the SOCKS5 discovery files (`socks5.port` + `socks5.auth`) on
/// daemon exit so the next `sunbeam vpn status` doesn't see stale creds.
struct SocksGuard {
    state_dir: std::path::PathBuf,
}

impl SocksGuard {
    fn new(state_dir: std::path::PathBuf) -> Self {
        Self { state_dir }
    }
}

impl Drop for SocksGuard {
    fn drop(&mut self) {
        crate::proxy::socks::SocksServer::remove_discovery_files(&self.state_dir);
    }
}

/// Run a single VPN session. Returns when the session ends (error or shutdown).
async fn run_session(
    config: &VpnConfig,
    keys: &crate::keys::NodeKeys,
    status: &Arc<RwLock<DaemonStatus>>,
    daemon_shutdown: &tokio_util::sync::CancellationToken,
) -> std::result::Result<SessionExit, crate::Error> {
    // 1. Connect to coordination server
    set_status(status, DaemonStatus::Connecting);
    let mut control = crate::control::ControlClient::connect(config, keys)
        .await
        .map_err(|e| {
            eprintln!("[session] connect: {e:?}");
            e
        })?;

    // 2. Register
    set_status(status, DaemonStatus::Registering);
    let _reg = control
        .register(&config.auth_key, &config.hostname, keys)
        .await?;

    // 2a. Send a Lite endpoint update so Headscale persists our DiscoKey on
    //     the node record. The streaming /machine/map handler doesn't
    //     update DiscoKey at capability versions ≥ 68 — only the Lite path
    //     does, and without it our peers can't see us in their netmaps.
    //
    //     We don't yet know our preferred DERP region on this first call —
    //     the DERPMap arrives with the first netmap below — so NetInfo is
    //     left unset here and re-sent with the correct region in step 4b.
    control
        .lite_update(keys, &config.hostname, None, None)
        .await?;

    // 3. Start map stream
    let mut map_stream = control.map_stream(keys, &config.hostname, None).await?;

    // 4. Wait for first netmap to get our addresses and peers
    let first_update = map_stream
        .next()
        .await?
        .ok_or_else(|| crate::Error::Control("map stream closed before first update".into()))?;

    let (peers, addresses, derp_map) = match &first_update {
        MapUpdate::Full(full) => {
            let addrs: Vec<IpAddr> = full
                .self_node
                .addresses
                .iter()
                .filter_map(|a| a.split('/').next()?.parse().ok())
                .collect();
            (full.peers.clone(), addrs, full.derp_map.clone())
        }
        _ => {
            return Err(crate::Error::Control(
                "expected Full netmap as first update".into(),
            ));
        }
    };

    let peer_count = peers.len();

    // 4b. Re-send the Lite endpoint update with NetInfo.PreferredDERP set
    //     to the first region from the just-received DERPMap. Headscale
    //     derives our `Node.DERP` field from this value, and peers use it
    //     to decide which relay region to address our packets to. Without
    //     it, our node record advertises "127.3.3.40:0" and DERP-only peers
    //     (the common case for peers behind NAT with unreachable endpoints)
    //     never route traffic to us, so WireGuard handshakes dead-end and
    //     time out at REKEY_ATTEMPT. The `map_stream` future above holds
    //     the original ControlClient hostage, so we open a fresh control
    //     connection just for this lite-update — one extra Noise handshake
    //     per session is a small price for correct DERP addressing.
    if let Some(ref dm) = derp_map
        && let Some(region) = dm.regions.values().next()
    {
        tracing::info!(
            "sending lite-update with preferred_derp={}",
            region.region_id
        );
        let mut lite_client = crate::control::ControlClient::connect(config, keys).await?;
        lite_client
            .lite_update(keys, &config.hostname, None, Some(region.region_id.into()))
            .await?;
        // Drop lite_client → closes its h2 connection. The map stream
        // on the original ControlClient is unaffected.
    }

    // 4a. Build the peer route table from the first netmap. Phase 1: the
    //     table is kept up-to-date but not yet consulted by the TCP proxy
    //     — Phase 2 (SOCKS5) will call `RouteTable::resolve` to pick an
    //     owning peer for arbitrary destination IPs. We maintain it now so
    //     the plumbing is in place and the behavior is testable.
    let route_table = {
        let mut t = RouteTable::new();
        t.rebuild(&peers, &config.route_whitelist);
        Arc::new(RwLock::new(t))
    };
    if let Ok(rt) = route_table.read() {
        tracing::info!(
            "route table initialized with {} entries from {} peers",
            rt.len(),
            peer_count
        );
    }

    // 5. Initialize WireGuard tunnel. Tailscale uses the node_key as the
    //    WireGuard static key — they are the same key, not separate. Peers
    //    only know our node_public from the netmap, so boringtun must be
    //    signing with the matching private key or peers will drop our
    //    handshakes for failing mac1 validation.
    let mut wg_tunnel = WgTunnel::new(keys.node_private.clone());
    wg_tunnel.update_peers(&peers);

    // 6. Set up NetworkEngine with our VPN IP
    let local_ip = addresses
        .first()
        .ok_or_else(|| crate::Error::Control("no addresses assigned".into()))?;
    let smoltcp_ip = match local_ip {
        IpAddr::V4(v4) => IpAddress::Ipv4(*v4),
        IpAddr::V6(v6) => IpAddress::Ipv6(*v6),
    };

    let (engine, channels) = NetworkEngine::new(smoltcp_ip, 10)?;

    // 7. Start TCP proxy that routes through the engine. Resolution chain
    //    for the cluster API target, in order:
    //      1. `cluster_api_host` — look up the peer by hostname in the netmap.
    //      2. First non-self peer address from the netmap (single-cluster
    //         deployments have exactly one peer and this is always right).
    //      3. `cluster_api_addr` — caller-provided explicit fallback.
    //    If all three fail, we skip the k8s proxy listener entirely and
    //    log a warning; the daemon still runs (SOCKS5 proxy, routes, etc.)
    //    but `sunbeam-sdk`'s kube-client VPN rerouting will see no proxy
    //    on 16579 and fall back to its default kubeconfig.
    let cancel = tokio_util::sync::CancellationToken::new();
    let proxy_cmd_tx = channels.cmd_tx.clone();
    let proxy_bind = config.proxy_bind;

    let auto_cluster_api = || -> Option<IpAddr> {
        // Look for a routed service CIDR in peers' allowed_ips. The k8s API
        // server is conventionally at the first IP in the service CIDR
        // (e.g. 10.43.0.1 for 10.43.0.0/16). We prefer service-CIDR routes
        // (prefix < /32) over the peer's own tailnet IP which typically has
        // nothing listening on 6443.
        for peer in &peers {
            for aip in &peer.allowed_ips {
                if let Ok(net) = aip.parse::<ipnet::IpNet>() {
                    // Skip host routes (/32 and /128) — those are tailnet IPs.
                    let is_host = matches!(
                        net,
                        ipnet::IpNet::V4(v4) if v4.prefix_len() == 32
                    ) || matches!(
                        net,
                        ipnet::IpNet::V6(v6) if v6.prefix_len() == 128
                    );
                    if is_host {
                        continue;
                    }
                    // First IP in the CIDR = network + 1 (the API server).
                    let network = net.network();
                    let api_ip = match network {
                        IpAddr::V4(v4) => {
                            let Some(bits) = u32::from(v4).checked_add(1) else {
                                continue;
                            };
                            IpAddr::V4(std::net::Ipv4Addr::from(bits))
                        }
                        IpAddr::V6(v6) => {
                            let Some(bits) = u128::from(v6).checked_add(1) else {
                                continue;
                            };
                            IpAddr::V6(std::net::Ipv6Addr::from(bits))
                        }
                    };
                    tracing::info!(
                        "auto-detected k8s API from peer route {aip} → {api_ip}"
                    );
                    return Some(api_ip);
                }
            }
        }
        None
    };

    let resolved_addr: Option<IpAddr> = if let Some(host) = config.cluster_api_host.as_deref() {
        match resolve_peer_ip(host, &peers) {
            Some(addr) => {
                tracing::info!("resolved cluster_api_host '{host}' → {addr}");
                Some(addr)
            }
            None => {
                let fallback = auto_cluster_api().or(config.cluster_api_addr);
                match fallback {
                    Some(addr) => tracing::warn!(
                        "cluster_api_host '{host}' did not match any netmap peer; \
                         falling back to {addr}"
                    ),
                    None => tracing::warn!(
                        "cluster_api_host '{host}' did not match any netmap peer \
                         and no fallback is available"
                    ),
                }
                fallback
            }
        }
    } else {
        auto_cluster_api().or(config.cluster_api_addr)
    };

    let proxy_cancel = cancel.clone();
    let cluster_api_port = config.cluster_api_port;
    let proxy_task = tokio::spawn(async move {
        match resolved_addr {
            Some(addr) => {
                let cluster_addr = std::net::SocketAddr::new(addr, cluster_api_port);
                tracing::info!("k8s proxy target: {cluster_addr}");
                run_proxy_listener(proxy_bind, cluster_addr, proxy_cmd_tx, proxy_cancel).await
            }
            None => {
                tracing::warn!(
                    "no cluster API target available (no cluster_api_host, no peers, \
                     no cluster_api_addr) — k8s proxy listener disabled"
                );
                // Park until cancellation so the task handle lifetime
                // matches the other session tasks.
                proxy_cancel.cancelled().await;
                Ok(())
            }
        }
    });

    // 7a. SOCKS5 + HTTP CONNECT proxy: loopback-only, auth-gated, enforces
    //     destination ACLs via the peer route table. Published port +
    //     auth token are written to `{state_dir}/socks5.{port,auth}` so
    //     downstream tools can point HTTPS_PROXY at it.
    let socks_cfg = crate::proxy::socks::SocksConfig {
        bind: config.socks_bind,
        allow_ports: config.socks_allow_ports.clone(),
        state_dir: config.state_dir.clone(),
    };
    // Build the optional cluster DNS resolver. It runs its queries
    // through the same engine command channel the SOCKS proxy uses,
    // so virtual TCP to the DNS server traverses the existing
    // smoltcp + WireGuard path.
    let resolver = config.dns_server.map(|dns_server| {
        std::sync::Arc::new(crate::dns::resolver::Resolver::new(
            dns_server,
            channels.cmd_tx.clone(),
            config.dns_search_domains.clone(),
        ))
    });
    // Shared audit log for the SOCKS proxy. The same Arc is handed to
    // the IpcServer below so `sunbeam vpn status` can tail it without
    // any extra plumbing.
    let audit_log = Arc::new(AuditLog::new());
    // Service registry watcher. The dns-controller (rc5+) writes
    // `{state_dir}/services.json` on reconcile; we watch for atomic
    // replaces and expose the current list via IPC + SOCKS slug
    // resolution. Missing file = empty registry, nothing breaks.
    let registry_watcher = crate::discovery::RegistryWatcher::spawn(
        config.state_dir.join("services.json"),
        cancel.clone(),
    );
    let discovery = registry_watcher.registry();
    let (socks_server, socks_endpoint) = crate::proxy::socks::SocksServer::bind(
        socks_cfg,
        route_table.clone(),
        channels.cmd_tx.clone(),
        resolver,
        audit_log.clone(),
        discovery.clone(),
    )
    .await?;
    let socks_state_dir = config.state_dir.clone();
    let _socks_guard = SocksGuard::new(socks_state_dir);
    let socks_cancel = cancel.clone();
    let socks_task = tokio::spawn(async move { socks_server.run(socks_cancel).await });
    let socks_proxy_port = Some(socks_endpoint.port);

    // 8. Engine task: polls smoltcp and bridges connections
    let engine_cancel = cancel.clone();
    let engine_task = tokio::spawn(async move {
        engine.run(engine_cancel).await;
    });

    // 9. Set up multi-region DERP manager.
    //
    //    The DerpManager handles per-region connections with lazy connect,
    //    reconnection on failure, route learning, and stale cleanup. A driver
    //    task owns the manager and bridges it to the WG loop via channels.
    let (derp_out_tx, derp_out_rx) = mpsc::channel::<([u8; 32], Vec<u8>)>(256);
    let (derp_in_tx, derp_in_rx) = mpsc::channel::<([u8; 32], Vec<u8>)>(256);

    let coord_scheme = if config.coordination_url.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let tls_mode = if config.derp_tls_insecure {
        DerpTlsMode::InsecureSkipVerify
    } else {
        DerpTlsMode::Verify
    };

    // Determine home region from derp_map.
    let home_region = derp_map
        .as_ref()
        .and_then(|dm| dm.regions.values().next())
        .map(|r| r.region_id)
        .unwrap_or(1);

    // Build the DerpManager with the DerpMap (or empty if none).
    let effective_derp_map = derp_map.clone().unwrap_or_default();

    // The manager's inbound channel delivers (region_id, src_key, data)
    // from per-region tasks. The driver bridges this to the WG loop.
    let (derp_mgr_in_tx, derp_mgr_in_rx) =
        mpsc::channel::<(u16, [u8; 32], Vec<u8>)>(256);

    let derp_manager = DerpManager::new(
        home_region,
        effective_derp_map.clone(),
        std::sync::Arc::new(keys.clone()),
        tls_mode,
        coord_scheme,
        derp_mgr_in_tx,
        cancel.clone(),
    );

    let (derp_cmd_tx, derp_cmd_rx) = mpsc::channel::<DerpCmd>(16);
    let derp_cancel = cancel.clone();
    let _derp_mgr_task = tokio::spawn(async move {
        run_derp_manager_task(derp_manager, derp_out_rx, derp_mgr_in_rx, derp_in_tx, derp_cmd_rx, derp_cancel).await;
    });

    // 9a. Bind a UDP socket for direct WireGuard transport. Failure here is
    //     non-fatal — DERP can carry traffic alone, just slower.
    let (udp_out_tx, udp_out_rx) = mpsc::channel::<(std::net::SocketAddr, Vec<u8>)>(256);
    let (udp_in_tx, udp_in_rx) = mpsc::channel::<(std::net::SocketAddr, Vec<u8>)>(256);
    let udp_socket: Option<std::sync::Arc<tokio::net::UdpSocket>> =
        match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
            Ok(socket) => {
                let local = socket.local_addr().ok();
                tracing::info!("WG UDP socket bound on {local:?}");
                Some(std::sync::Arc::new(socket))
            }
            Err(e) => {
                tracing::warn!("UDP bind failed: {e}; continuing with DERP only");
                None
            }
        };
    let _udp_task = if let Some(ref sock) = udp_socket {
        let socket = sock.clone();
        let udp_cancel = cancel.clone();
        Some(tokio::spawn(async move {
            run_udp_loop(socket, udp_out_rx, udp_in_tx, udp_cancel).await;
        }))
    } else {
        None
    };

    // Channel for STUN-discovered endpoints → WG loop for CallMeMaybe dispatch.
    let (our_endpoints_tx, our_endpoints_rx) = mpsc::channel::<Vec<std::net::SocketAddr>>(4);

    // Broadcast channel for STUN responses. The WG loop's packet classifier
    // forwards any packet with the STUN magic cookie here so netcheck tasks
    // can read them without racing the WG loop for the shared UDP socket.
    let (stun_bcast_tx, _) =
        tokio::sync::broadcast::channel::<(std::net::SocketAddr, Vec<u8>)>(64);

    // 9b. Start network interface monitor. On macOS this uses an AF_ROUTE
    //     socket; on other platforms it's a no-op with a warning. On change,
    //     trigger a STUN re-probe and send the results to the DERP manager
    //     to update the home region.
    let _netmon = match crate::netmon::Monitor::new(cancel.clone()) {
        Ok(monitor) => {
            let mut rx = monitor.subscribe();
            let restun_derp_tx = derp_cmd_tx.clone();
            let restun_ep_tx = our_endpoints_tx.clone();
            let restun_socket = udp_socket.clone();
            let restun_derp_map = effective_derp_map.clone();
            let restun_bcast = stun_bcast_tx.clone();
            tokio::spawn(async move {
                let netcheck = crate::stun::netcheck::Client::new();
                while let Ok(delta) = rx.recv().await {
                    tracing::info!(
                        "network change: default_iface={} ips={} rebind={}",
                        delta.default_interface_changed,
                        delta.interface_ips_changed,
                        delta.rebind_likely_required,
                    );
                    if delta.rebind_likely_required
                        && let Some(ref sock) = restun_socket {
                            let mut stun_rx = restun_bcast.subscribe();
                            let report = netcheck
                                .report_mux(&restun_derp_map, sock, &mut stun_rx)
                                .await;
                            if report.preferred_derp != 0 {
                                tracing::info!("re-STUN: preferred_derp={}, global_v4={:?}", report.preferred_derp, report.global_v4);
                                let _ = restun_derp_tx.send(DerpCmd::SetHome(report.preferred_derp)).await;
                            }
                            let mut eps = Vec::new();
                            if let Some(v4) = report.global_v4 { eps.push(v4); }
                            if let Some(v6) = report.global_v6 { eps.push(v6); }
                            if !eps.is_empty() {
                                let _ = restun_ep_tx.send(eps).await;
                            }
                    }
                }
            });
            Some(monitor)
        }
        Err(e) => {
            tracing::warn!("netmon: failed to start: {e}");
            None
        }
    };

    // 9c. Initial STUN probe to discover our NAT-mapped address and preferred
    //     DERP region. Runs asynchronously so it doesn't block session setup.
    if let Some(ref sock) = udp_socket {
        let stun_socket = sock.clone();
        let stun_derp_map = effective_derp_map.clone();
        let stun_derp_tx = derp_cmd_tx.clone();
        let stun_ep_tx = our_endpoints_tx.clone();
        let stun_bcast_tx_init = stun_bcast_tx.clone();
        tokio::spawn(async move {
            let netcheck = crate::stun::netcheck::Client::new();
            let mut stun_rx = stun_bcast_tx_init.subscribe();
            let report = netcheck
                .report_mux(&stun_derp_map, &stun_socket, &mut stun_rx)
                .await;
            if report.udp {
                tracing::info!(
                    "initial STUN: preferred_derp={}, v4={:?}, v6={:?}, mapping_varies={:?}",
                    report.preferred_derp, report.global_v4, report.global_v6, report.mapping_varies,
                );
                if report.preferred_derp != 0 {
                    let _ = stun_derp_tx.send(DerpCmd::SetHome(report.preferred_derp)).await;
                }
                // Collect discovered endpoints and send to WG loop for CallMeMaybe.
                let mut eps = Vec::new();
                if let Some(v4) = report.global_v4 { eps.push(v4); }
                if let Some(v6) = report.global_v6 { eps.push(v6); }
                if !eps.is_empty() {
                    let _ = stun_ep_tx.send(eps).await;
                }
            } else {
                tracing::debug!("initial STUN: no UDP responses (behind strict NAT or STUN ports blocked)");
            }
        });
    }

    // 10. WG encap/decap task: bridges engine IP packets ↔ WG ↔ transport
    //     The peer_update channel carries netmap peer changes from
    //     map_stream_loop into the WG loop so update_peers() is called
    //     on every netmap change, not just at startup.
    let (peer_update_tx, peer_update_rx) = mpsc::channel::<Vec<Node>>(16);
    // Key-rotation signal: wg_loop → run_session. Capacity 1 — one pending
    // rotation is enough; extras are silently dropped via try_send.
    let (rotate_key_tx, mut rotate_key_rx) = mpsc::channel::<()>(1);
    let engine_to_wg_rx = channels.engine_to_wg_rx;
    let wg_to_engine_tx = channels.wg_to_engine_tx;
    let wg_cancel = cancel.clone();
    // Convert disco keys to crypto_box types for NaCl seal/open.
    let disco_secret = crypto_box::SecretKey::from(keys.disco_private.to_bytes());
    let my_disco_pub: [u8; 32] = *keys.disco_public.as_bytes();
    let wg_status = status.clone();
    let wg_initial_peers = peers.clone();
    let wg_task = tokio::spawn(async move {
        run_wg_loop(
            wg_tunnel,
            engine_to_wg_rx,
            wg_to_engine_tx,
            derp_out_tx,
            Some(derp_in_rx),
            udp_out_tx,
            Some(udp_in_rx),
            peer_update_rx,
            our_endpoints_rx,
            disco_secret,
            my_disco_pub,
            stun_bcast_tx.clone(),
            wg_status,
            rotate_key_tx,
            wg_cancel,
            wg_initial_peers,
        )
        .await
    });

    // 11. Start IPC server
    let ipc = IpcServer::new(
        &config.control_socket,
        status.clone(),
        route_table.clone(),
        audit_log.clone(),
        discovery.clone(),
        daemon_shutdown.clone(),
    )?;

    // Mark as ready
    let derp_home = derp_map
        .as_ref()
        .and_then(|dm| dm.regions.values().next())
        .map(|r| r.region_id);

    set_status(
        status,
        DaemonStatus::Running {
            addresses,
            peer_count,
            derp_home,
            socks_proxy_port,
            last_handshake_fail: None,
        },
    );

    // 11. Run concurrent tasks
    let route_whitelist = config.route_whitelist.clone();
    let state_dir = config.state_dir.clone();
    tokio::select! {
        result = map_stream_loop(&mut map_stream, status, &route_table, &route_whitelist, &peer_update_tx, &derp_cmd_tx) => {
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
        r = socks_task => {
            tracing::error!("socks_task exited: {r:?}");
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
        _ = rotate_key_rx.recv() => {
            tracing::info!("rotating node key after stuck-handshake watchdog escalation");
            cancel.cancel();
            match crate::keys::NodeKeys::rotate_node_key(&state_dir) {
                Ok(()) => Ok(SessionExit::RotatedKey),
                Err(e) => {
                    tracing::error!("node key rotation failed: {e:?}");
                    Ok(SessionExit::Disconnected)
                }
            }
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
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|e| crate::Error::Io {
            context: format!("bind proxy {bind_addr}"),
            source: e,
        })?;

    tracing::info!(
        "VPN proxy listening on {}",
        listener.local_addr().unwrap_or(bind_addr)
    );

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
/// and sends WG packets out via UDP (preferred) or DERP relay (fallback).
/// Receives WG packets from either transport, decapsulates them, and feeds
/// the resulting IP packets back to the engine.
#[allow(clippy::too_many_arguments)]
async fn run_wg_loop(
    mut tunnel: WgTunnel,
    mut from_engine: mpsc::Receiver<Vec<u8>>,
    to_engine: mpsc::Sender<Vec<u8>>,
    derp_out_tx: mpsc::Sender<([u8; 32], Vec<u8>)>,
    mut derp_in_rx: Option<mpsc::Receiver<([u8; 32], Vec<u8>)>>,
    udp_out_tx: mpsc::Sender<(std::net::SocketAddr, Vec<u8>)>,
    mut udp_in_rx: Option<mpsc::Receiver<(std::net::SocketAddr, Vec<u8>)>>,
    mut peer_update_rx: mpsc::Receiver<Vec<Node>>,
    mut our_endpoints_rx: mpsc::Receiver<Vec<std::net::SocketAddr>>,
    disco_private: crypto_box::SecretKey,
    my_disco_pub: [u8; 32],
    stun_response_tx: tokio::sync::broadcast::Sender<(std::net::SocketAddr, Vec<u8>)>,
    status: Arc<RwLock<DaemonStatus>>,
    rotate_key_tx: mpsc::Sender<()>,
    cancel: tokio_util::sync::CancellationToken,
    initial_peers: Vec<Node>,
) {
    let mut tick_interval = tokio::time::interval(Duration::from_millis(250));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Disco shared keys: peer_disco_pub → precomputed SalsaBox.
    // Seeded from the initial peer set so DERP-arrived disco packets can be
    // decrypted before the first Full netmap push fires.
    let mut disco_shared: std::collections::HashMap<[u8; 32], crypto_box::SalsaBox> =
        std::collections::HashMap::new();
    rebuild_disco_shared(&disco_private, &initial_peers, &mut disco_shared);
    let mut endpoint_tracker = crate::daemon::endpoint::EndpointTracker::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            ip_packet = from_engine.recv() => {
                match ip_packet {
                    Some(packet) => {
                        if let Some(dst_ip) = parse_dst_ip(&packet) {
                            let action = tunnel.encapsulate(dst_ip, &packet);
                            dispatch_encap(action, &derp_out_tx, &udp_out_tx).await;
                        }
                    }
                    None => return, // engine dropped
                }
            }
            incoming = async { derp_in_rx.as_mut()?.recv().await }, if derp_in_rx.is_some() => {
                match incoming {
                    Some((src_key, data)) => {
                        tracing::trace!("WG ← DERP ({} bytes)", data.len());
                        match crate::disco::packet::classify(&data) {
                            PacketKind::Disco => {
                                tracing::info!("disco packet from {:02x}{:02x}..{:02x}{:02x} via derp ({} bytes)", src_key[0], src_key[1], src_key[30], src_key[31], data.len());
                                if let Some((msg, sender_pub)) = crate::disco::open(&data, &disco_shared) {
                                    // No real socket addr for DERP-delivered packets; pong routing
                                    // uses sender_pub + derp_out_tx when via_derp=true.
                                    let derp_src_addr = std::net::SocketAddr::from(([0, 0, 0, 0], 0));
                                    handle_disco(
                                        msg, sender_pub, derp_src_addr,
                                        &mut endpoint_tracker, &mut tunnel,
                                        &my_disco_pub, &disco_shared,
                                        &derp_out_tx, &udp_out_tx,
                                        true,
                                    ).await;
                                } else {
                                    tracing::trace!("disco decrypt failed from DERP {:02x}{:02x}..", src_key[0], src_key[1]);
                                }
                            }
                            _ => {
                                // WireGuard (or unknown) packet — pass to tunnel.
                                let action = tunnel.decapsulate(&src_key, &data);
                                handle_decap(action, src_key, &tunnel, &to_engine, &derp_out_tx, &udp_out_tx).await;
                            }
                        }
                    }
                    None => {
                        tracing::warn!("DERP channel closed; continuing on UDP only");
                        derp_in_rx = None;
                    }
                }
            }
            incoming = async { udp_in_rx.as_mut()?.recv().await }, if udp_in_rx.is_some() => {
                match incoming {
                    Some((src_addr, data)) => {
                        match crate::disco::packet::classify(&data) {
                            PacketKind::Stun => {
                                tracing::trace!("STUN response from {src_addr} ({} bytes)", data.len());
                                let _ = stun_response_tx.send((src_addr, data));
                            }
                            PacketKind::Disco => {
                                tracing::info!("disco packet from {src_addr} via udp ({} bytes)", data.len());
                                if let Some((msg, sender_pub)) = crate::disco::open(&data, &disco_shared) {
                                    handle_disco(
                                        msg, sender_pub, src_addr,
                                        &mut endpoint_tracker, &mut tunnel,
                                        &my_disco_pub, &disco_shared,
                                        &derp_out_tx, &udp_out_tx,
                                        false,
                                    ).await;
                                } else {
                                    tracing::trace!("disco decrypt failed from {src_addr}");
                                }
                            }
                            PacketKind::WireGuard => {
                                tracing::trace!("WG ← UDP {src_addr} ({} bytes)", data.len());
                                let Some(peer_key) = identify_udp_peer(&tunnel, src_addr, &data) else {
                                    tracing::trace!("UDP packet from {src_addr}: no peer match");
                                    continue;
                                };
                                // Learn the peer's endpoint from received traffic.
                                tunnel.update_peer_endpoint(&peer_key, src_addr);
                                let action = tunnel.decapsulate(&peer_key, &data);
                                handle_decap(action, peer_key, &tunnel, &to_engine, &derp_out_tx, &udp_out_tx).await;
                            }
                            PacketKind::Unknown => {
                                tracing::trace!("unknown packet from {src_addr} ({} bytes)", data.len());
                            }
                        }
                    }
                    None => {
                        tracing::warn!("UDP channel closed; continuing on DERP only");
                        udp_in_rx = None;
                    }
                }
            }
            peers = peer_update_rx.recv() => {
                match peers {
                    Some(new_peers) => {
                        tracing::info!("WG tunnel: applying peer update ({} peers)", new_peers.len());
                        tunnel.update_peers(&new_peers);
                        // Rebuild disco shared keys from updated peer list.
                        rebuild_disco_shared(&disco_private, &new_peers, &mut disco_shared);
                    }
                    None => {
                        tracing::warn!("peer update channel closed");
                    }
                }
            }
            endpoints = our_endpoints_rx.recv() => {
                if let Some(eps) = endpoints {
                    tracing::info!("discovered endpoints: {eps:?}, sending CallMeMaybe to all peers");
                    // Send CallMeMaybe to each peer via DERP so they probe our endpoints.
                    for peer_key in tunnel.peer_keys() {
                        let cmm = endpoint_tracker.build_call_me_maybe(&peer_key, &eps);
                        let msg = crate::disco::Message::CallMeMaybe(cmm);
                        if let Some(shared) = disco_shared.get(&peer_key) {
                            let sealed = crate::disco::seal(&msg, &my_disco_pub, shared);
                            let _ = derp_out_tx.send((peer_key, sealed)).await;
                        }
                    }
                }
            }
            _ = tick_interval.tick() => {
                let tick_result = tunnel.tick();
                for ta in tick_result.actions {
                    dispatch_encap(ta.action, &derp_out_tx, &udp_out_tx).await;
                }
                if tick_result.had_connection_expired {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    update_last_handshake_fail(&status, ts);
                }
                if tick_result.needs_key_rotation {
                    tracing::warn!("stuck-handshake watchdog: escalating to node-key rotation");
                    let _ = rotate_key_tx.try_send(());
                }
            }
        }
    }
}

/// Dispatch a WG encap action to whichever transports it carries. We send
/// over both UDP and DERP when both are populated; the remote peer dedupes
/// duplicate ciphertexts via the WireGuard replay window.
async fn dispatch_encap(
    action: crate::wg::tunnel::EncapAction,
    derp_out_tx: &mpsc::Sender<([u8; 32], Vec<u8>)>,
    udp_out_tx: &mpsc::Sender<(std::net::SocketAddr, Vec<u8>)>,
) {
    if let Some((endpoint, data)) = action.udp {
        tracing::trace!("WG → UDP {endpoint} ({} bytes)", data.len());
        let _ = udp_out_tx.send((endpoint, data)).await;
    }
    if let Some((dest_key, data)) = action.derp {
        tracing::trace!("WG → DERP ({} bytes)", data.len());
        let _ = derp_out_tx.send((dest_key, data)).await;
    }
}

/// Handle a single decapsulation result regardless of which transport it
/// arrived on. Decrypted IP packets go to the engine; handshake responses
/// go back out via both DERP and UDP (using the peer's known endpoint).
async fn handle_decap(
    action: DecapAction,
    peer_key: [u8; 32],
    tunnel: &WgTunnel,
    to_engine: &mpsc::Sender<Vec<u8>>,
    derp_out_tx: &mpsc::Sender<([u8; 32], Vec<u8>)>,
    udp_out_tx: &mpsc::Sender<(std::net::SocketAddr, Vec<u8>)>,
) {
    match action {
        DecapAction::Packet(p) => {
            tracing::trace!("decap → engine ({} bytes)", p.len());
            if to_engine.send(p).await.is_err() {
                tracing::warn!("engine channel closed — packet dropped");
            }
        }
        DecapAction::Response(responses) => {
            for r in responses {
                tracing::debug!(
                    "decap ��� response ({} bytes, type={})",
                    r.len(),
                    if r.is_empty() { 0 } else { r[0] }
                );
                let action = tunnel.route_to_peer(&peer_key, r);
                dispatch_encap(action, derp_out_tx, udp_out_tx).await;
            }
        }
        DecapAction::Nothing => {}
    }
}

/// Handle an incoming disco message (already decrypted).
///
/// `src_addr` is the observed socket address of the sender. For packets that
/// arrived via DERP (signalled by `via_derp = true`) this is a sentinel
/// `0.0.0.0:0`; routing uses the sender's disco key via `derp_out_tx` instead.
#[allow(clippy::too_many_arguments)]
async fn handle_disco(
    msg: crate::disco::Message,
    sender_disco_pub: [u8; 32],
    src_addr: std::net::SocketAddr,
    endpoint_tracker: &mut crate::daemon::endpoint::EndpointTracker,
    tunnel: &mut WgTunnel,
    my_disco_pub: &[u8; 32],
    disco_shared: &std::collections::HashMap<[u8; 32], crypto_box::SalsaBox>,
    derp_out_tx: &mpsc::Sender<([u8; 32], Vec<u8>)>,
    udp_out_tx: &mpsc::Sender<(std::net::SocketAddr, Vec<u8>)>,
    via_derp: bool,
) {
    match msg {
        crate::disco::Message::Ping(ping) => {
            tracing::info!(
                "disco Ping from {:02x}{:02x}.. via {} tx={:02x}{:02x}{:02x}{:02x}",
                sender_disco_pub[0], sender_disco_pub[1],
                if via_derp { "derp" } else { "udp" },
                ping.tx_id[0], ping.tx_id[1], ping.tx_id[2], ping.tx_id[3],
            );
            // Reply with a Pong carrying the observed source address.
            let pong = crate::disco::Message::Pong(crate::disco::Pong {
                tx_id: ping.tx_id,
                src: src_addr,
            });
            if let Some(shared) = disco_shared.get(&sender_disco_pub) {
                let sealed = crate::disco::seal(&pong, my_disco_pub, shared);
                // Send pong back on the same transport the ping arrived on.
                if via_derp {
                    let _ = derp_out_tx.send((sender_disco_pub, sealed)).await;
                } else {
                    let _ = udp_out_tx.send((src_addr, sealed)).await;
                }
            }
        }
        crate::disco::Message::Pong(pong) => {
            tracing::info!(
                "disco Pong from {:02x}{:02x}.. via {} observed={} tx={:02x}{:02x}{:02x}{:02x}",
                sender_disco_pub[0], sender_disco_pub[1],
                if via_derp { "derp" } else { "udp" },
                pong.src,
                pong.tx_id[0], pong.tx_id[1], pong.tx_id[2], pong.tx_id[3],
            );
            // Find the node key for this disco key so we can update the tunnel.
            // For now we use the disco pub as an approximation — the endpoint
            // tracker maps disco keys internally.
            if let Some(best) = endpoint_tracker.handle_pong(&sender_disco_pub, &pong.tx_id, pong.src) {
                tracing::info!(
                    "peer {:02x}{:02x}.. best direct addr: {best}",
                    sender_disco_pub[0], sender_disco_pub[1],
                );
                tunnel.update_peer_endpoint(&sender_disco_pub, best);
            }
        }
        crate::disco::Message::CallMeMaybe(cmm) => {
            tracing::info!(
                "disco CallMeMaybe from {:02x}{:02x}.. with {} endpoints",
                sender_disco_pub[0], sender_disco_pub[1],
                cmm.endpoints.len(),
            );
            // Probe the advertised endpoints.
            let pings = endpoint_tracker.handle_call_me_maybe(&sender_disco_pub, cmm.endpoints);
            if let Some(shared) = disco_shared.get(&sender_disco_pub) {
                for (addr, ping) in pings {
                    let msg = crate::disco::Message::Ping(ping);
                    let sealed = crate::disco::seal(&msg, my_disco_pub, shared);
                    let _ = udp_out_tx.send((addr, sealed)).await;
                }
            }
        }
    }
}

/// Rebuild the disco shared key table from a peer list.
///
/// Each peer's `disco_key` field (if present) is parsed into a 32-byte public
/// key, and a `SalsaBox` is precomputed from our disco private key and the
/// peer's disco public key.
fn rebuild_disco_shared(
    disco_private: &crypto_box::SecretKey,
    peers: &[Node],
    shared: &mut std::collections::HashMap<[u8; 32], crypto_box::SalsaBox>,
) {
    shared.clear();
    for peer in peers {
        if let Some(dk) = parse_disco_key(&peer.disco_key) {
            let peer_pub = crypto_box::PublicKey::from(dk);
            let sb = crypto_box::SalsaBox::new(&peer_pub, disco_private);
            shared.insert(dk, sb);
        }
    }
    tracing::debug!("rebuilt disco shared keys for {} peers", shared.len());
}

/// Parse a "discokey:<hex>" string into raw 32 bytes.
fn parse_disco_key(s: &str) -> Option<[u8; 32]> {
    let hex_str = s.strip_prefix("discokey:")?;
    let mut bytes = [0u8; 32];
    if hex_str.len() != 64 {
        return None;
    }
    for i in 0..32 {
        bytes[i] = u8::from_str_radix(&hex_str[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

/// Identify which peer a UDP-delivered WireGuard packet belongs to.
///
/// WireGuard message types 2 (HandshakeResponse), 3 (CookieReply), and 4
/// (TransportData) all carry a `receiver_index` at bytes 4..8 — that's the
/// boringtun local index we assigned when adding the peer, so we can look
/// it up directly. Type 1 (HandshakeInitiation) doesn't carry it; for
/// those we fall back to matching the source address against advertised
/// peer endpoints.
fn identify_udp_peer(
    tunnel: &WgTunnel,
    src_addr: std::net::SocketAddr,
    packet: &[u8],
) -> Option<[u8; 32]> {
    if packet.len() < 8 {
        return None;
    }
    let msg_type = packet[0];
    match msg_type {
        2..=4 => {
            let idx = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
            tunnel
                .find_peer_by_local_index(idx)
                .or_else(|| tunnel.find_peer_by_endpoint(src_addr))
        }
        _ => tunnel.find_peer_by_endpoint(src_addr),
    }
}

/// Bridge a UDP socket to the WG layer via mpsc channels.
async fn run_udp_loop(
    socket: std::sync::Arc<tokio::net::UdpSocket>,
    mut out_rx: mpsc::Receiver<(std::net::SocketAddr, Vec<u8>)>,
    in_tx: mpsc::Sender<(std::net::SocketAddr, Vec<u8>)>,
    cancel: tokio_util::sync::CancellationToken,
) {
    // Spawn a dedicated recv task — UdpSocket supports concurrent send+recv
    // when shared via Arc, but mixing both in one select! makes lifetimes
    // awkward, so split them.
    let recv_socket = socket.clone();
    let recv_cancel = cancel.clone();
    let recv_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            tokio::select! {
                _ = recv_cancel.cancelled() => return,
                result = recv_socket.recv_from(&mut buf) => {
                    match result {
                        Ok((n, src)) => {
                            if in_tx.send((src, buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("UDP recv error: {e}");
                            return;
                        }
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                recv_task.abort();
                return;
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some((dst, data)) => {
                        if let Err(e) = socket.send_to(&data, dst).await {
                            tracing::warn!("UDP send to {dst} failed: {e}");
                        }
                    }
                    None => {
                        recv_task.abort();
                        return;
                    }
                }
            }
        }
    }
}

// run_derp_loop removed — replaced by run_derp_manager_task + DerpManager

/// Commands sent to the DERP manager driver task.
enum DerpCmd {
    /// Update the DerpMap (from a new netmap push).
    UpdateMap(crate::proto::types::DerpMap),
    /// Change the home region (from netcheck results).
    SetHome(u16),
}

/// Drive the DERP manager: bridge outbound packets from the WG loop,
/// forward inbound packets (with route learning), handle DerpMap/home
/// updates, and periodically clean stale non-home region connections.
async fn run_derp_manager_task(
    mut manager: DerpManager,
    mut out_rx: mpsc::Receiver<([u8; 32], Vec<u8>)>,
    mut mgr_in_rx: mpsc::Receiver<(u16, [u8; 32], Vec<u8>)>,
    in_tx: mpsc::Sender<([u8; 32], Vec<u8>)>,
    mut cmd_rx: mpsc::Receiver<DerpCmd>,
    cancel: tokio_util::sync::CancellationToken,
) {
    manager.ensure_home().await;

    let mut clean_interval = tokio::time::interval(Duration::from_secs(15));
    clean_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                manager.shutdown().await;
                return;
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some((dest_key, data)) => {
                        manager.send_to_peer(&dest_key, data).await;
                    }
                    None => {
                        manager.shutdown().await;
                        return;
                    }
                }
            }
            incoming = mgr_in_rx.recv() => {
                match incoming {
                    Some((region_id, src_key, data)) => {
                        manager.learn_peer_route(&src_key, region_id);
                        if in_tx.send((src_key, data)).await.is_err() {
                            return;
                        }
                    }
                    None => return,
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(DerpCmd::UpdateMap(map)) => {
                        manager.update_derp_map(map);
                    }
                    Some(DerpCmd::SetHome(region)) => {
                        manager.set_home_region(region).await;
                    }
                    None => {}
                }
            }
            _ = clean_interval.tick() => {
                manager.clean_stale();
            }
        }
    }
}

/// Look up a peer's tailnet IP from the netmap by hostname.
///
/// Tries (in order): exact hostname match, exact `name` (FQDN) match,
/// then prefix match against `name`. Returns the first IPv4 address
/// from the peer's `addresses` list, falling back to IPv6 only if
/// there are no v4 entries.
fn resolve_peer_ip(host: &str, peers: &[crate::proto::types::Node]) -> Option<IpAddr> {
    let matched = peers
        .iter()
        .find(|p| p.hostinfo.hostname == host)
        .or_else(|| peers.iter().find(|p| p.name == host))
        .or_else(|| peers.iter().find(|p| p.name.starts_with(host)))?;

    let addrs: Vec<IpAddr> = matched
        .addresses
        .iter()
        .filter_map(|s| s.split('/').next()?.parse().ok())
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .copied()
        .or_else(|| addrs.first().copied())
}

/// Extract host:port from a coordination URL like `http://localhost:8080`.
#[cfg(test)]
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
///
/// This also keeps the shared [`RouteTable`] in sync with the netmap so
/// Phase 2 (SOCKS5 resolution) can consult it. Rebuilds on `Full`, upserts
/// on `PeersChanged`, and removes on `PeersRemoved`.
async fn map_stream_loop(
    stream: &mut crate::control::MapStream,
    status: &Arc<RwLock<DaemonStatus>>,
    route_table: &Arc<RwLock<RouteTable>>,
    route_whitelist: &[ipnet::IpNet],
    peer_update_tx: &mpsc::Sender<Vec<Node>>,
    derp_cmd_tx: &mpsc::Sender<DerpCmd>,
) -> crate::Result<()> {
    loop {
        match stream.next().await? {
            Some(update) => match &update {
                MapUpdate::Full(full) => {
                    update_peer_count(status, full.peers.len());
                    if let Ok(mut rt) = route_table.write() {
                        rt.rebuild(&full.peers, route_whitelist);
                        tracing::info!("netmap: {} peers, {} routes", full.peers.len(), rt.len());
                    } else {
                        tracing::info!("netmap: {} peers", full.peers.len());
                    }
                    // Forward peer list to the WG tunnel so it can update
                    // endpoints, add new peers, and drop stale ones.
                    if peer_update_tx.send(full.peers.clone()).await.is_err() {
                        tracing::warn!("peer update channel closed — WG loop may be dead");
                    }
                    // Forward DerpMap to the DERP manager if it changed.
                    if let Some(ref dm) = full.derp_map {
                        let _ = derp_cmd_tx.send(DerpCmd::UpdateMap(dm.clone())).await;
                    }
                }
                MapUpdate::PeersChanged(peers) => {
                    if let Ok(mut rt) = route_table.write() {
                        rt.apply_changed(peers, route_whitelist);
                    }
                    tracing::debug!("netmap: {} peers changed", peers.len());
                    // PeersChanged is a delta — forward the changed subset.
                    // update_peers handles upsert for known keys and insert
                    // for new ones; it won't remove peers not in this list
                    // (that's PeersRemoved's job). However, update_peers
                    // retains only keys in the input set. So we need to send
                    // changed peers through a different path or accumulate.
                    // For now, the Full update (which Headscale sends on any
                    // material change) is the primary trigger. PeersChanged
                    // is logged but not forwarded — this is safe because
                    // Headscale always follows PeersChanged with a Full.
                }
                MapUpdate::PeersRemoved(keys) => {
                    if let Ok(mut rt) = route_table.write() {
                        rt.apply_removed(keys);
                    }
                    tracing::debug!("netmap: {} peers removed", keys.len());
                }
                MapUpdate::KeepAlive => {
                    tracing::trace!("netmap: keep-alive");
                }
            },
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
    if let Ok(mut s) = status.write()
        && let DaemonStatus::Running { peer_count, .. } = &mut *s
    {
        *peer_count = count;
    }
}

fn update_last_handshake_fail(status: &Arc<RwLock<DaemonStatus>>, ts_secs: u64) {
    if let Ok(mut s) = status.write()
        && let DaemonStatus::Running { last_handshake_fail, .. } = &mut *s
    {
        *last_handshake_fail = Some(ts_secs);
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
            cluster_api_addr: Some("10.0.0.1".parse().unwrap()),
            cluster_api_port: 6443,
            cluster_api_host: None,
            control_socket: dir.path().join("test.sock"),
            hostname: "test-node".to_string(),
            server_public_key: Some([0xaa; 32]),
            derp_tls_insecure: false,
            route_whitelist: crate::config::default_route_whitelist(),
            socks_bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            socks_allow_ports: crate::config::default_socks_allow_ports(),
            dns_server: None,
            dns_search_domains: vec![],
        };

        let handle = VpnDaemon::start(config).await.unwrap();

        // Give the daemon a moment to attempt connection and enter reconnecting state.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let status = handle.current_status();
        // It should be reconnecting (connection to bogus addr fails) or connecting.
        match status {
            DaemonStatus::Reconnecting { .. } | DaemonStatus::Connecting => {}
            other => panic!("expected Reconnecting or Connecting, got {other:?}"),
        }

        handle.shutdown().await.unwrap();
    }

    #[test]
    fn test_parse_dst_ip_v4() {
        // Minimal valid IPv4 header (20 bytes), version=4, dst=10.43.0.1
        let mut pkt = [0u8; 20];
        pkt[0] = 0x45; // version 4, IHL 5
        pkt[16] = 10;
        pkt[17] = 43;
        pkt[18] = 0;
        pkt[19] = 1;
        assert_eq!(
            parse_dst_ip(&pkt),
            Some(IpAddr::V4(std::net::Ipv4Addr::new(10, 43, 0, 1)))
        );
    }

    #[test]
    fn test_parse_dst_ip_v6() {
        let mut pkt = [0u8; 40];
        pkt[0] = 0x60; // version 6
        // dst at bytes 24..40: ::1
        pkt[39] = 1;
        assert_eq!(
            parse_dst_ip(&pkt),
            Some(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST))
        );
    }

    #[test]
    fn test_parse_dst_ip_truncated() {
        assert_eq!(parse_dst_ip(&[]), None);
        assert_eq!(parse_dst_ip(&[0x45; 19]), None); // v4 too short
        assert_eq!(parse_dst_ip(&[0x60; 39]), None); // v6 too short
        assert_eq!(parse_dst_ip(&[0x30; 20]), None); // bad version
    }

    #[test]
    fn test_coordination_host_port_https() {
        let (h, p) = coordination_host_port("https://headscale.sunbeam.pt:8443").unwrap();
        assert_eq!(h, "headscale.sunbeam.pt");
        assert_eq!(p, 8443);
    }

    #[test]
    fn test_coordination_host_port_default() {
        let (h, p) = coordination_host_port("https://headscale.sunbeam.pt").unwrap();
        assert_eq!(h, "headscale.sunbeam.pt");
        assert_eq!(p, 443);

        let (h, p) = coordination_host_port("http://localhost").unwrap();
        assert_eq!(h, "localhost");
        assert_eq!(p, 80);
    }

    #[test]
    fn test_coordination_host_port_with_path() {
        let (h, p) = coordination_host_port("https://headscale.sunbeam.pt:8443/ts2021").unwrap();
        assert_eq!(h, "headscale.sunbeam.pt");
        assert_eq!(p, 8443);
    }

    #[test]
    fn test_identify_udp_peer_type2() {
        // Type 2 (HandshakeResponse) — receiver_index at bytes 4..8.
        let (_, secret) = {
            let s = x25519_dalek::StaticSecret::random_from_rng(rand::rngs::OsRng);
            let p = x25519_dalek::PublicKey::from(&s);
            (*p.as_bytes(), s)
        };
        let mut tunnel = WgTunnel::new(secret);

        let (peer_pub, _) = {
            let s = x25519_dalek::StaticSecret::random_from_rng(rand::rngs::OsRng);
            let p = x25519_dalek::PublicKey::from(&s);
            (*p.as_bytes(), s)
        };
        let node = crate::proto::types::Node {
            id: 1,
            key: format!(
                "nodekey:{}",
                peer_pub.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ),
            disco_key: "discokey:0000000000000000000000000000000000000000000000000000000000000000".into(),
            addresses: vec!["100.64.0.2/32".into()],
            allowed_ips: vec!["100.64.0.0/24".into()],
            endpoints: vec!["1.2.3.4:41641".into()],
            derp: "127.3.3.40:1".into(),
            hostinfo: crate::proto::types::HostInfo {
                go_arch: "arm64".into(),
                go_os: "linux".into(),
                go_version: "test".into(),
                hostname: "test".into(),
                os: "linux".into(),
                os_version: "6.1".into(),
                device_model: None,
                frontend_log_id: None,
                backend_log_id: None,
                net_info: None,
            },
            name: "peer.test".into(),
            online: Some(true),
            machine_authorized: true,
        };
        tunnel.update_peers(&[node]);

        // Build a fake type-2 packet with local_index=0 at bytes 4..8.
        let mut pkt = vec![0u8; 92]; // HandshakeResponse is 92 bytes
        pkt[0] = 2; // type 2
        pkt[4..8].copy_from_slice(&0u32.to_le_bytes()); // local index 0
        let addr: std::net::SocketAddr = "5.6.7.8:9999".parse().unwrap();
        let found = identify_udp_peer(&tunnel, addr, &pkt);
        assert_eq!(found, Some(peer_pub));
    }

    #[test]
    fn test_identify_udp_peer_too_short() {
        let secret = x25519_dalek::StaticSecret::random_from_rng(rand::rngs::OsRng);
        let tunnel = WgTunnel::new(secret);
        let addr: std::net::SocketAddr = "1.2.3.4:5678".parse().unwrap();
        assert_eq!(identify_udp_peer(&tunnel, addr, &[1, 2, 3]), None);
    }

    #[test]
    fn test_parse_disco_key_valid() {
        let hex = "discokey:0102030405060708091011121314151617181920212223242526272829303132";
        let result = parse_disco_key(hex).unwrap();
        assert_eq!(result[0], 0x01);
        assert_eq!(result[31], 0x32);
    }

    #[test]
    fn test_parse_disco_key_no_prefix() {
        assert!(parse_disco_key("nodekey:aabb").is_none());
    }

    #[test]
    fn test_parse_disco_key_empty() {
        assert!(parse_disco_key("").is_none());
        assert!(parse_disco_key("discokey:").is_none());
    }
}
