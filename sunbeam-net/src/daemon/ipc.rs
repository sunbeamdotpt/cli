//! IPC server/client for daemon control.

// IPC server/client for daemon control.
//
// The IPC interface uses a Unix domain socket at the configured
// control_socket path. Commands include:
// - Status: query current daemon status
// - Routes: dump the subnet-router table
// - RecentConnections: tail the SOCKS proxy audit log
// - Reconnect: force reconnection to coordination server
// - Stop: gracefully shut down the daemon

use std::path::Path;
use std::sync::{Arc, RwLock};

use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use super::state::DaemonStatus;
use crate::control::RouteTable;
use crate::discovery::{ServiceEntry, ServiceRegistry};
use crate::proxy::audit::{AuditEntry, AuditLog};

/// IPC command sent to the daemon.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    /// Query the current daemon status.
    Status,
    /// Dump the current subnet-router table.
    Routes,
    /// Query the last N audit entries. `max` caps the response size;
    /// the daemon may return fewer if the ring buffer is smaller.
    RecentConnections { max: usize },
    /// Force reconnection.
    Reconnect,
    /// Dump the current service registry. Empty until the dns-controller
    /// writes `~/.sunbeam/vpn/services.json`.
    Services,
    /// Gracefully stop the daemon.
    Stop,
}

/// One entry in the `Routes` response — a wire-format view of a
/// single `(cidr, node_key)` pair from the peer route table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RouteInfo {
    /// The advertised prefix, rendered as a CIDR string (`10.42.0.0/16`).
    pub cidr: String,
    /// Node key of the peer advertising the prefix.
    pub node_key: String,
}

/// IPC response from the daemon.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcResponse {
    /// Status response.
    Status(DaemonStatus),
    /// Subnet-router table, longest prefix first.
    Routes(Vec<RouteInfo>),
    /// Tail of the proxy audit log, newest first.
    RecentConnections(Vec<AuditEntry>),
    /// Service registry snapshot.
    Services(Vec<ServiceEntry>),
    /// Acknowledgement.
    Ok,
    /// Error.
    Error(String),
}

/// Unix domain socket IPC server for daemon control.
pub(crate) struct IpcServer {
    listener: UnixListener,
    status: Arc<RwLock<DaemonStatus>>,
    /// Peer route table. Shared with the map-stream loop (writes) and
    /// the SOCKS proxy (reads). The IPC server only ever reads from it.
    routes: Arc<RwLock<RouteTable>>,
    /// Ring-buffer audit log shared with the SOCKS proxy.
    audit: Arc<AuditLog>,
    /// Service registry, maintained by [`RegistryWatcher`]. Empty until
    /// the dns-controller writes `services.json`.
    discovery: Arc<RwLock<ServiceRegistry>>,
    /// Cancellation token shared with the daemon loop. Cancelling this from
    /// an IPC `Stop` request triggers graceful shutdown of the whole
    /// daemon, the same as `DaemonHandle::shutdown()`.
    daemon_shutdown: CancellationToken,
}

impl IpcServer {
    /// Bind a new IPC server at the given socket path.
    pub fn new(
        socket_path: &Path,
        status: Arc<RwLock<DaemonStatus>>,
        routes: Arc<RwLock<RouteTable>>,
        audit: Arc<AuditLog>,
        discovery: Arc<RwLock<ServiceRegistry>>,
        daemon_shutdown: CancellationToken,
    ) -> crate::Result<Self> {
        // Remove stale socket file if it exists.
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path).map_err(|e| crate::Error::Io {
            context: "bind IPC socket".into(),
            source: e,
        })?;
        Ok(Self {
            listener,
            status,
            routes,
            audit,
            discovery,
            daemon_shutdown,
        })
    }

    /// Accept and handle IPC connections until cancelled.
    pub async fn run(&self) -> crate::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await.map_err(|e| crate::Error::Io {
                context: "accept IPC".into(),
                source: e,
            })?;
            let status = self.status.clone();
            let routes = self.routes.clone();
            let audit = self.audit.clone();
            let discovery = self.discovery.clone();
            let shutdown = self.daemon_shutdown.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_ipc_connection(
                    stream, &status, &routes, &audit, &discovery, &shutdown,
                )
                .await
                {
                    tracing::warn!("IPC error: {e}");
                }
            });
        }
    }
}

/// Client for talking to a running VPN daemon over its Unix control socket.
pub struct IpcClient {
    socket_path: std::path::PathBuf,
}

impl IpcClient {
    /// Build a client targeting the given control socket. No connection is
    /// established until a request method is called.
    pub fn new(socket_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Returns true if the control socket file exists. (Doesn't prove the
    /// daemon is alive — only that something has bound there at some point.)
    pub fn socket_exists(&self) -> bool {
        self.socket_path.exists()
    }

    /// Send a single command and return the response.
    pub async fn request(&self, cmd: IpcCommand) -> crate::Result<IpcResponse> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;

        let stream =
            UnixStream::connect(&self.socket_path)
                .await
                .map_err(|e| crate::Error::Io {
                    context: format!("connect to {}", self.socket_path.display()),
                    source: e,
                })?;
        let (reader, mut writer) = stream.into_split();

        let mut req_bytes = serde_json::to_vec(&cmd)?;
        req_bytes.push(b'\n');
        writer
            .write_all(&req_bytes)
            .await
            .map_err(|e| crate::Error::Io {
                context: "write IPC request".into(),
                source: e,
            })?;
        writer.shutdown().await.map_err(|e| crate::Error::Io {
            context: "shutdown IPC writer".into(),
            source: e,
        })?;

        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| crate::Error::Io {
                context: "read IPC response".into(),
                source: e,
            })?;
        if n == 0 {
            return Err(crate::Error::Ipc(
                "daemon closed the connection without responding".into(),
            ));
        }

        let resp: IpcResponse = serde_json::from_str(line.trim())?;
        Ok(resp)
    }

    /// Convenience: query the daemon's status.
    pub async fn status(&self) -> crate::Result<DaemonStatus> {
        match self.request(IpcCommand::Status).await? {
            IpcResponse::Status(s) => Ok(s),
            IpcResponse::Error(e) => Err(crate::Error::Ipc(e)),
            other => Err(crate::Error::Ipc(format!(
                "unexpected response to Status: {other:?}"
            ))),
        }
    }

    /// Convenience: tell the daemon to shut down. Returns once the daemon
    /// has acknowledged the request — the daemon process may take a moment
    /// longer to actually exit.
    pub async fn stop(&self) -> crate::Result<()> {
        match self.request(IpcCommand::Stop).await? {
            IpcResponse::Ok => Ok(()),
            IpcResponse::Error(e) => Err(crate::Error::Ipc(e)),
            other => Err(crate::Error::Ipc(format!(
                "unexpected response to Stop: {other:?}"
            ))),
        }
    }

    /// Convenience: dump the daemon's current subnet-router table.
    pub async fn routes(&self) -> crate::Result<Vec<RouteInfo>> {
        match self.request(IpcCommand::Routes).await? {
            IpcResponse::Routes(list) => Ok(list),
            IpcResponse::Error(e) => Err(crate::Error::Ipc(e)),
            other => Err(crate::Error::Ipc(format!(
                "unexpected response to Routes: {other:?}"
            ))),
        }
    }

    /// Convenience: tail the SOCKS proxy audit log, newest first.
    pub async fn recent_connections(&self, max: usize) -> crate::Result<Vec<AuditEntry>> {
        match self.request(IpcCommand::RecentConnections { max }).await? {
            IpcResponse::RecentConnections(list) => Ok(list),
            IpcResponse::Error(e) => Err(crate::Error::Ipc(e)),
            other => Err(crate::Error::Ipc(format!(
                "unexpected response to RecentConnections: {other:?}"
            ))),
        }
    }

    /// Convenience: dump the daemon's current service registry.
    pub async fn services(&self) -> crate::Result<Vec<ServiceEntry>> {
        match self.request(IpcCommand::Services).await? {
            IpcResponse::Services(list) => Ok(list),
            IpcResponse::Error(e) => Err(crate::Error::Ipc(e)),
            other => Err(crate::Error::Ipc(format!(
                "unexpected response to Services: {other:?}"
            ))),
        }
    }
}

async fn handle_ipc_connection(
    stream: tokio::net::UnixStream,
    status: &Arc<RwLock<DaemonStatus>>,
    routes: &Arc<RwLock<RouteTable>>,
    audit: &Arc<AuditLog>,
    discovery: &Arc<RwLock<ServiceRegistry>>,
    daemon_shutdown: &CancellationToken,
) -> crate::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| crate::Error::Io {
            context: "read IPC".into(),
            source: e,
        })?;

    let cmd: IpcCommand = serde_json::from_str(line.trim())?;

    let response = match cmd {
        IpcCommand::Status => {
            let s = status
                .read()
                .map_err(|e| crate::Error::Ipc(e.to_string()))?;
            IpcResponse::Status(s.clone())
        }
        IpcCommand::Routes => {
            let r = routes
                .read()
                .map_err(|e| crate::Error::Ipc(e.to_string()))?;
            let list = r
                .routes()
                .map(|(net, key)| RouteInfo {
                    cidr: net.to_string(),
                    node_key: key.to_string(),
                })
                .collect();
            IpcResponse::Routes(list)
        }
        IpcCommand::RecentConnections { max } => {
            IpcResponse::RecentConnections(audit.snapshot(max))
        }
        IpcCommand::Reconnect => {
            // TODO: implement targeted session reconnect (without dropping
            // the daemon). For now treat it the same as a no-op.
            IpcResponse::Ok
        }
        IpcCommand::Services => {
            let r = discovery
                .read()
                .map_err(|e| crate::Error::Ipc(e.to_string()))?;
            IpcResponse::Services(r.entries().to_vec())
        }
        IpcCommand::Stop => {
            daemon_shutdown.cancel();
            IpcResponse::Ok
        }
    };

    let mut resp_bytes = serde_json::to_vec(&response)?;
    resp_bytes.push(b'\n');
    writer
        .write_all(&resp_bytes)
        .await
        .map_err(|e| crate::Error::Io {
            context: "write IPC".into(),
            source: e,
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::types::{HostInfo, Node};
    use crate::proxy::audit::AuditOutcome;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    fn empty_routes() -> Arc<RwLock<RouteTable>> {
        Arc::new(RwLock::new(RouteTable::new()))
    }

    fn empty_audit() -> Arc<AuditLog> {
        Arc::new(AuditLog::new())
    }

    fn empty_discovery() -> Arc<RwLock<ServiceRegistry>> {
        Arc::new(RwLock::new(ServiceRegistry::new()))
    }

    #[tokio::test]
    async fn test_ipc_status_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");

        let status = Arc::new(RwLock::new(DaemonStatus::Running {
            addresses: vec!["100.64.0.1".parse().unwrap()],
            peer_count: 3,
            derp_home: Some(1),
            socks_proxy_port: None,
            last_handshake_fail: None,
        }));

        let server = IpcServer::new(
            &sock_path,
            status,
            empty_routes(),
            empty_audit(),
            empty_discovery(),
            CancellationToken::new(),
        )
        .unwrap();
        let server_task = tokio::spawn(async move { server.run().await });

        // Give the server a moment to start accepting.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect and send a Status command.
        let mut stream = UnixStream::connect(&sock_path).await.unwrap();
        let cmd = serde_json::to_string(&IpcCommand::Status).unwrap();
        stream
            .write_all(format!("{cmd}\n").as_bytes())
            .await
            .unwrap();

        let (reader, _writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await.unwrap();

        let resp: IpcResponse = serde_json::from_str(response_line.trim()).unwrap();
        match resp {
            IpcResponse::Status(DaemonStatus::Running { peer_count, .. }) => {
                assert_eq!(peer_count, 3);
            }
            other => panic!("expected Status(Running), got {other:?}"),
        }

        server_task.abort();
    }

    #[tokio::test]
    async fn test_ipc_unknown_command_handling() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");

        let status = Arc::new(RwLock::new(DaemonStatus::Stopped));
        let server = IpcServer::new(
            &sock_path,
            status,
            empty_routes(),
            empty_audit(),
            empty_discovery(),
            CancellationToken::new(),
        )
        .unwrap();
        let server_task = tokio::spawn(async move { server.run().await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send malformed JSON.
        let mut stream = UnixStream::connect(&sock_path).await.unwrap();
        stream.write_all(b"not valid json\n").await.unwrap();

        // The server should handle this gracefully (log warning, close connection).
        // The connection should close without the server crashing.
        let (reader, _writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut response_line = String::new();
        // Read will return 0 bytes (EOF) since the handler errors and drops the connection.
        let n = reader.read_line(&mut response_line).await.unwrap();
        assert_eq!(n, 0, "expected EOF after malformed command");

        // Server should still be running — send a valid command on a new connection.
        let mut stream2 = UnixStream::connect(&sock_path).await.unwrap();
        let cmd = serde_json::to_string(&IpcCommand::Status).unwrap();
        stream2
            .write_all(format!("{cmd}\n").as_bytes())
            .await
            .unwrap();

        let (reader2, _) = stream2.into_split();
        let mut reader2 = BufReader::new(reader2);
        let mut resp_line = String::new();
        reader2.read_line(&mut resp_line).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(resp_line.trim()).unwrap();
        assert!(matches!(resp, IpcResponse::Status(DaemonStatus::Stopped)));

        server_task.abort();
    }

    fn test_node(key: &str, cidrs: &[&str]) -> Node {
        Node {
            id: 0,
            key: key.to_string(),
            disco_key: format!("discokey:{key}"),
            addresses: vec![],
            allowed_ips: cidrs.iter().map(|s| (*s).to_string()).collect(),
            endpoints: vec![],
            derp: "127.3.3.40:1".to_string(),
            hostinfo: HostInfo::default(),
            name: format!("{key}.test"),
            online: Some(true),
            machine_authorized: true,
        }
    }

    async fn send_cmd(sock_path: &std::path::Path, cmd: IpcCommand) -> IpcResponse {
        let mut stream = UnixStream::connect(sock_path).await.unwrap();
        let body = serde_json::to_string(&cmd).unwrap();
        stream
            .write_all(format!("{body}\n").as_bytes())
            .await
            .unwrap();
        let (reader, _writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    #[tokio::test]
    async fn test_ipc_routes_query_returns_table_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");

        let status = Arc::new(RwLock::new(DaemonStatus::Stopped));
        let routes = {
            let mut t = RouteTable::new();
            t.rebuild(
                &[
                    test_node("nodekey:api", &["10.42.0.0/16"]),
                    test_node("nodekey:wide", &["10.0.0.0/8"]),
                ],
                &["10.0.0.0/8".parse().unwrap()],
            );
            Arc::new(RwLock::new(t))
        };

        let server = IpcServer::new(
            &sock_path,
            status,
            routes,
            empty_audit(),
            empty_discovery(),
            CancellationToken::new(),
        )
        .unwrap();
        let server_task = tokio::spawn(async move { server.run().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let resp = send_cmd(&sock_path, IpcCommand::Routes).await;
        match resp {
            IpcResponse::Routes(list) => {
                assert_eq!(list.len(), 2, "got {list:?}");
                // Longest prefix first.
                assert_eq!(list[0].cidr, "10.42.0.0/16");
                assert_eq!(list[0].node_key, "nodekey:api");
                assert_eq!(list[1].cidr, "10.0.0.0/8");
                assert_eq!(list[1].node_key, "nodekey:wide");
            }
            other => panic!("expected Routes, got {other:?}"),
        }
        server_task.abort();
    }

    #[tokio::test]
    async fn test_ipc_routes_empty_when_no_peers() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");
        let status = Arc::new(RwLock::new(DaemonStatus::Stopped));
        let server = IpcServer::new(
            &sock_path,
            status,
            empty_routes(),
            empty_audit(),
            empty_discovery(),
            CancellationToken::new(),
        )
        .unwrap();
        let server_task = tokio::spawn(async move { server.run().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        match send_cmd(&sock_path, IpcCommand::Routes).await {
            IpcResponse::Routes(list) => assert!(list.is_empty()),
            other => panic!("expected Routes, got {other:?}"),
        }
        server_task.abort();
    }

    #[tokio::test]
    async fn test_ipc_recent_connections_returns_audit_tail() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");
        let status = Arc::new(RwLock::new(DaemonStatus::Stopped));

        let audit = Arc::new(AuditLog::new());
        audit.record(
            "socks5",
            "10.42.0.1:443".to_string(),
            AuditOutcome::Accepted,
        );
        audit.record("http", "blocked:9999".to_string(), AuditOutcome::DeniedPort);
        audit.record(
            "socks5",
            "postgres.data.svc:5432".to_string(),
            AuditOutcome::Accepted,
        );

        let server = IpcServer::new(
            &sock_path,
            status,
            empty_routes(),
            audit,
            empty_discovery(),
            CancellationToken::new(),
        )
        .unwrap();
        let server_task = tokio::spawn(async move { server.run().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Ask for the last 10 — we only have 3.
        match send_cmd(&sock_path, IpcCommand::RecentConnections { max: 10 }).await {
            IpcResponse::RecentConnections(entries) => {
                assert_eq!(entries.len(), 3);
                // Newest first.
                assert_eq!(entries[0].destination, "postgres.data.svc:5432");
                assert_eq!(entries[0].outcome, AuditOutcome::Accepted);
                assert_eq!(entries[1].destination, "blocked:9999");
                assert_eq!(entries[1].outcome, AuditOutcome::DeniedPort);
                assert_eq!(entries[2].destination, "10.42.0.1:443");
            }
            other => panic!("expected RecentConnections, got {other:?}"),
        }

        // And cap it to 2.
        match send_cmd(&sock_path, IpcCommand::RecentConnections { max: 2 }).await {
            IpcResponse::RecentConnections(entries) => assert_eq!(entries.len(), 2),
            other => panic!("expected RecentConnections, got {other:?}"),
        }

        server_task.abort();
    }
}
