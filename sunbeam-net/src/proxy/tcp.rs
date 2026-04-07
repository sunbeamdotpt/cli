use std::net::SocketAddr;
use tokio::net::TcpListener;

/// TCP proxy that accepts local connections and forwards them through the VPN.
///
/// Currently uses direct TCP as a placeholder until the WireGuard virtual
/// network is wired in.
pub(crate) struct TcpProxy {
    bind_addr: SocketAddr,
    remote_addr: SocketAddr,
}

impl TcpProxy {
    pub fn new(bind_addr: SocketAddr, remote_addr: SocketAddr) -> Self {
        Self { bind_addr, remote_addr }
    }

    /// Run the proxy, accepting connections until the cancellation token fires.
    pub async fn run(&self, cancel: tokio_util::sync::CancellationToken) -> crate::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await
            .map_err(|e| crate::Error::Io {
                context: format!("bind proxy {}", self.bind_addr),
                source: e,
            })?;

        tracing::info!("TCP proxy listening on {}", self.bind_addr);

        loop {
            tokio::select! {
                accept = listener.accept() => {
                    let (stream, peer) = accept
                        .map_err(|e| crate::Error::Io { context: "accept proxy".into(), source: e })?;
                    tracing::debug!("proxy connection from {peer}");
                    let remote = self.remote_addr;
                    tokio::spawn(async move {
                        if let Err(e) = handle_proxy_connection(stream, remote).await {
                            tracing::debug!("proxy connection error: {e}");
                        }
                    });
                }
                _ = cancel.cancelled() => {
                    tracing::info!("TCP proxy shutting down");
                    return Ok(());
                }
            }
        }
    }
}

/// Handle a single proxied connection.
/// TODO: Route through WireGuard virtual network instead of direct TCP.
async fn handle_proxy_connection(
    mut local: tokio::net::TcpStream,
    remote_addr: SocketAddr,
) -> crate::Result<()> {
    // For now, direct TCP connect as placeholder.
    // When wg/socket.rs is ready, this will dial through the virtual network.
    let mut remote = tokio::net::TcpStream::connect(remote_addr).await
        .map_err(|e| crate::Error::Io {
            context: format!("connect to {remote_addr}"),
            source: e,
        })?;

    tokio::io::copy_bidirectional(&mut local, &mut remote).await
        .map_err(|e| crate::Error::Io {
            context: "proxy copy".into(),
            source: e,
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn test_proxy_bind_and_accept() {
        let cancel = tokio_util::sync::CancellationToken::new();

        // Use port 0 for OS-assigned port; we need to know the actual port.
        // Start a dummy remote server so the proxy connection doesn't fail.
        let remote_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote_addr = remote_listener.local_addr().unwrap();

        // Bind a temporary listener to find a free port, then release it.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        drop(listener);

        let proxy = TcpProxy::new(proxy_addr, remote_addr);
        let proxy_cancel = cancel.clone();
        let proxy_task = tokio::spawn(async move { proxy.run(proxy_cancel).await });

        // Give it a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect to the proxy.
        let stream = TcpStream::connect(proxy_addr).await.unwrap();
        assert!(stream.peer_addr().is_ok());

        // Accept on the remote side so the proxy connection completes.
        let _ = remote_listener.accept().await.unwrap();

        cancel.cancel();
        let _ = proxy_task.await;
    }

    #[tokio::test]
    async fn test_proxy_forwarding() {
        let cancel = tokio_util::sync::CancellationToken::new();

        // Start a mock echo server.
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo_listener.local_addr().unwrap();

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = echo_listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    loop {
                        let n = match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        if stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        // Start the proxy pointing at the echo server.
        // Find a free port for the proxy.
        let tmp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = tmp_listener.local_addr().unwrap();
        drop(tmp_listener);

        let proxy = TcpProxy::new(proxy_addr, echo_addr);
        let proxy_cancel = cancel.clone();
        tokio::spawn(async move { proxy.run(proxy_cancel).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect through the proxy and send data.
        let mut stream = TcpStream::connect(proxy_addr).await.unwrap();

        let payload = b"hello through the proxy";
        stream.write_all(payload).await.unwrap();

        let mut buf = vec![0u8; payload.len()];
        stream.read_exact(&mut buf).await.unwrap();

        assert_eq!(&buf, payload);

        cancel.cancel();
    }
}
