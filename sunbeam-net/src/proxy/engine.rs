//! NetworkEngine: bridges async I/O with smoltcp's synchronous polling.
//!
//! The engine owns the virtual TCP/IP stack (smoltcp) and manages proxy
//! connections that tunnel through WireGuard. It runs as a single async
//! task with a poll loop.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use smoltcp::wire::IpAddress;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::wg::socket::VirtualNetwork;

/// Commands sent to the engine from the proxy listener.
pub(crate) enum EngineCommand {
    /// A new local TCP connection to proxy through the VPN.
    NewConnection {
        local: TcpStream,
        remote: SocketAddr,
    },
}

/// The network engine bridges local TCP connections through the smoltcp
/// virtual network and WireGuard tunnel.
pub(crate) struct NetworkEngine {
    vnet: VirtualNetwork,
    /// Receive commands (new connections).
    cmd_rx: mpsc::Receiver<EngineCommand>,
    /// IP packets coming from WireGuard (decrypted) → smoltcp device.
    _wg_to_smoltcp_tx: mpsc::Sender<Vec<u8>>,
    /// IP packets from smoltcp device → to be WireGuard encrypted.
    smoltcp_to_wg_rx: mpsc::Receiver<Vec<u8>>,
    /// Send encrypted WG IP packets outward (to the daemon for routing).
    outbound_tx: mpsc::Sender<Vec<u8>>,
    /// Active proxy connections.
    connections: HashMap<u64, ProxyConnection>,
    next_id: u64,
}

struct ProxyConnection {
    local: TcpStream,
    handle: crate::wg::socket::TcpSocketHandle,
    /// Buffer for data read from local TCP, waiting to be sent to smoltcp.
    local_buf: Vec<u8>,
    /// Buffer for data read from smoltcp, waiting to be written to local TCP.
    remote_buf: Vec<u8>,
    /// Whether the local read side is done (EOF or error).
    local_read_done: bool,
    /// Whether the remote (smoltcp) side is done.
    remote_done: bool,
}

/// Channels for wiring the engine to the WireGuard layer.
pub(crate) struct EngineChannels {
    /// Send IP packets into the engine (from WG decap).
    pub wg_to_engine_tx: mpsc::Sender<Vec<u8>>,
    /// Receive IP packets from the engine (for WG encap).
    pub engine_to_wg_rx: mpsc::Receiver<Vec<u8>>,
    /// Send commands to the engine.
    pub cmd_tx: mpsc::Sender<EngineCommand>,
}

impl NetworkEngine {
    /// Create a new engine with the given local VPN IP address.
    ///
    /// Returns the engine and a set of channels for communicating with it.
    pub fn new(local_ip: IpAddress, prefix_len: u8) -> crate::Result<(Self, EngineChannels)> {
        // Channels between WG and smoltcp device
        let (wg_to_smoltcp_tx, wg_to_smoltcp_rx) = mpsc::channel::<Vec<u8>>(256);
        let (smoltcp_to_wg_tx, smoltcp_to_wg_rx) = mpsc::channel::<Vec<u8>>(256);

        // Channel for outbound IP packets (engine → daemon for WG encap)
        let (outbound_tx, engine_to_wg_rx) = mpsc::channel::<Vec<u8>>(256);

        // Command channel
        let (cmd_tx, cmd_rx) = mpsc::channel(32);

        let vnet = VirtualNetwork::new(local_ip, prefix_len, wg_to_smoltcp_rx, smoltcp_to_wg_tx)?;

        let engine = Self {
            vnet,
            cmd_rx,
            _wg_to_smoltcp_tx: wg_to_smoltcp_tx.clone(),
            smoltcp_to_wg_rx,
            outbound_tx,
            connections: HashMap::new(),
            next_id: 0,
        };

        let channels = EngineChannels {
            wg_to_engine_tx: wg_to_smoltcp_tx,
            engine_to_wg_rx,
            cmd_tx,
        };

        Ok((engine, channels))
    }

    /// Run the engine poll loop. This should be spawned as a tokio task.
    pub async fn run(mut self, cancel: tokio_util::sync::CancellationToken) {
        let mut poll_interval = tokio::time::interval(Duration::from_millis(5));
        poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::debug!("network engine shutting down");
                    return;
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(EngineCommand::NewConnection { local, remote }) => {
                            self.add_connection(local, remote);
                        }
                        None => return, // command channel closed
                    }
                }
                _ = poll_interval.tick() => {
                    self.poll_cycle().await;
                }
            }
        }
    }

    fn add_connection(&mut self, local: TcpStream, remote: SocketAddr) {
        match self.vnet.tcp_connect(remote) {
            Ok(handle) => {
                let id = self.next_id;
                self.next_id += 1;
                tracing::debug!("proxy connection {id} → {remote}");
                self.connections.insert(
                    id,
                    ProxyConnection {
                        local,
                        handle,
                        local_buf: Vec::with_capacity(8192),
                        remote_buf: Vec::with_capacity(8192),
                        local_read_done: false,
                        remote_done: false,
                    },
                );
            }
            Err(e) => {
                tracing::warn!("failed to open virtual TCP to {remote}: {e}");
            }
        }
    }

    async fn poll_cycle(&mut self) {
        // 1. Forward outbound IP packets from smoltcp → WG
        while let Ok(ip_packet) = self.smoltcp_to_wg_rx.try_recv() {
            let _ = self.outbound_tx.try_send(ip_packet);
        }

        // 2. Poll smoltcp
        self.vnet.poll();

        // 3. Bridge each proxy connection
        let mut to_remove = Vec::new();
        let conn_ids: Vec<u64> = self.connections.keys().copied().collect();

        for id in conn_ids {
            let conn = self.connections.get_mut(&id).unwrap();
            let done = Self::bridge_connection(&mut self.vnet, conn).await;
            if done {
                to_remove.push(id);
            }
        }

        for id in to_remove {
            tracing::debug!("proxy connection {id} closed");
            self.connections.remove(&id);
        }

        // 4. Poll again after bridging to flush any new outbound data
        self.vnet.poll();

        // Drain outbound again
        while let Ok(ip_packet) = self.smoltcp_to_wg_rx.try_recv() {
            let _ = self.outbound_tx.try_send(ip_packet);
        }
    }

    /// Bridge data between a local TCP stream and a smoltcp socket.
    /// Returns true if the connection is done and should be removed.
    async fn bridge_connection(vnet: &mut VirtualNetwork, conn: &mut ProxyConnection) -> bool {
        // Local → smoltcp: try to read from local TCP (non-blocking)
        if !conn.local_read_done && conn.local_buf.len() < 32768 {
            let mut tmp = [0u8; 8192];
            match conn.local.try_read(&mut tmp) {
                Ok(0) => {
                    conn.local_read_done = true;
                }
                Ok(n) => {
                    conn.local_buf.extend_from_slice(&tmp[..n]);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    conn.local_read_done = true;
                }
            }
        }

        // Send buffered local data to smoltcp socket
        if !conn.local_buf.is_empty() {
            match vnet.tcp_send(conn.handle, &conn.local_buf) {
                Ok(n) if n > 0 => {
                    conn.local_buf.drain(..n);
                }
                _ => {}
            }
        }

        // smoltcp → local: read from smoltcp socket. Errors here mean the
        // socket has closed cleanly (FIN received and drained); the
        // SynSent/Listen/etc transient states return Ok(0) instead.
        let mut tmp = [0u8; 8192];
        match vnet.tcp_recv(conn.handle, &mut tmp) {
            Ok(n) if n > 0 => {
                tracing::trace!("bridge: smoltcp → buf {n} bytes");
                conn.remote_buf.extend_from_slice(&tmp[..n]);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("bridge: smoltcp recv ended: {e}");
                conn.remote_done = true;
            }
        }

        // Write buffered smoltcp data to local TCP
        if !conn.remote_buf.is_empty() {
            match conn.local.try_write(&conn.remote_buf) {
                Ok(n) if n > 0 => {
                    tracing::trace!("bridge: buf → local {n} bytes");
                    conn.remote_buf.drain(..n);
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    tracing::debug!("bridge: local write error: {e}");
                }
            }
        }

        // Check if the virtual socket is still alive
        if !vnet.tcp_is_active(conn.handle) && conn.remote_buf.is_empty() {
            conn.remote_done = true;
        }

        // Done when the remote side has finished AND we've flushed all of
        // its data to the local socket. We don't wait for the local side
        // to half-close its write, because most clients (curl, kubectl,
        // browsers) keep the write side open until they see EOF on the
        // read side. Returning true here drops the local TcpStream, which
        // closes the connection from our end and lets the client read EOF.
        conn.remote_done && conn.remote_buf.is_empty()
    }
}
