// IPC server/client for daemon control.
//
// The IPC interface uses a Unix domain socket at the configured
// control_socket path. Commands include:
// - Status: query current daemon status
// - Reconnect: force reconnection to coordination server
// - Stop: gracefully shut down the daemon

use std::path::Path;
use std::sync::{Arc, RwLock};

use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;

use super::state::DaemonStatus;

/// IPC command sent to the daemon.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcCommand {
    /// Query the current daemon status.
    Status,
    /// Force reconnection.
    Reconnect,
    /// Gracefully stop the daemon.
    Stop,
}

/// IPC response from the daemon.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcResponse {
    /// Status response.
    Status(DaemonStatus),
    /// Acknowledgement.
    Ok,
    /// Error.
    Error(String),
}

/// Unix domain socket IPC server for daemon control.
pub(crate) struct IpcServer {
    listener: UnixListener,
    status: Arc<RwLock<DaemonStatus>>,
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
        daemon_shutdown: CancellationToken,
    ) -> crate::Result<Self> {
        // Remove stale socket file if it exists.
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path)
            .map_err(|e| crate::Error::Io { context: "bind IPC socket".into(), source: e })?;
        Ok(Self {
            listener,
            status,
            daemon_shutdown,
        })
    }

    /// Accept and handle IPC connections until cancelled.
    pub async fn run(&self) -> crate::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await
                .map_err(|e| crate::Error::Io { context: "accept IPC".into(), source: e })?;
            let status = self.status.clone();
            let shutdown = self.daemon_shutdown.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_ipc_connection(stream, &status, &shutdown).await {
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

        let stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            crate::Error::Io {
                context: format!("connect to {}", self.socket_path.display()),
                source: e,
            }
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
        writer
            .shutdown()
            .await
            .map_err(|e| crate::Error::Io {
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
}

async fn handle_ipc_connection(
    stream: tokio::net::UnixStream,
    status: &Arc<RwLock<DaemonStatus>>,
    daemon_shutdown: &CancellationToken,
) -> crate::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await
        .map_err(|e| crate::Error::Io { context: "read IPC".into(), source: e })?;

    let cmd: IpcCommand = serde_json::from_str(line.trim())?;

    let response = match cmd {
        IpcCommand::Status => {
            let s = status.read().map_err(|e| crate::Error::Ipc(e.to_string()))?;
            IpcResponse::Status(s.clone())
        }
        IpcCommand::Reconnect => {
            // TODO: implement targeted session reconnect (without dropping
            // the daemon). For now treat it the same as a no-op.
            IpcResponse::Ok
        }
        IpcCommand::Stop => {
            daemon_shutdown.cancel();
            IpcResponse::Ok
        }
    };

    let mut resp_bytes = serde_json::to_vec(&response)?;
    resp_bytes.push(b'\n');
    writer.write_all(&resp_bytes).await
        .map_err(|e| crate::Error::Io { context: "write IPC".into(), source: e })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn test_ipc_status_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let sock_path = dir.path().join("test.sock");

        let status = Arc::new(RwLock::new(DaemonStatus::Running {
            addresses: vec!["100.64.0.1".parse().unwrap()],
            peer_count: 3,
            derp_home: Some(1),
        }));

        let server = IpcServer::new(&sock_path, status, CancellationToken::new()).unwrap();
        let server_task = tokio::spawn(async move { server.run().await });

        // Give the server a moment to start accepting.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect and send a Status command.
        let mut stream = UnixStream::connect(&sock_path).await.unwrap();
        let cmd = serde_json::to_string(&IpcCommand::Status).unwrap();
        stream.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();

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
        let server = IpcServer::new(&sock_path, status, CancellationToken::new()).unwrap();
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
        stream2.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();

        let (reader2, _) = stream2.into_split();
        let mut reader2 = BufReader::new(reader2);
        let mut resp_line = String::new();
        reader2.read_line(&mut resp_line).await.unwrap();
        let resp: IpcResponse = serde_json::from_str(resp_line.trim()).unwrap();
        assert!(matches!(resp, IpcResponse::Status(DaemonStatus::Stopped)));

        server_task.abort();
    }
}
