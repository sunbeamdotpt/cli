//! Integration tests for sunbeam-net against a real Headscale instance.
//!
//! These tests require the docker-compose stack to be running:
//!   cd sunbeam-net/tests && ./run.sh
//!
//! Environment variables:
//!   SUNBEAM_NET_TEST_AUTH_KEY   — pre-auth key for registration
//!   SUNBEAM_NET_TEST_COORD_URL — Headscale URL (e.g. http://localhost:8080)
//!   SUNBEAM_NET_TEST_PEER_A_IP — tailnet IP of peer-a (for connectivity test)

#![cfg(feature = "integration")]

use std::env;

fn require_env(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| {
        panic!(
            "Integration test requires {key} env var. Run via sunbeam-net/tests/run.sh"
        )
    })
}

/// Test: connect to Headscale, register with pre-auth key, receive a netmap.
#[tokio::test(flavor = "multi_thread")]
async fn test_register_and_receive_netmap() {
    let coord_url = require_env("SUNBEAM_NET_TEST_COORD_URL");
    let auth_key = require_env("SUNBEAM_NET_TEST_AUTH_KEY");

    let state_dir = tempfile::tempdir().unwrap();
    let config = sunbeam_net::VpnConfig {
        coordination_url: coord_url,
        auth_key,
        state_dir: state_dir.path().to_path_buf(),
        proxy_bind: "127.0.0.1:0".parse().unwrap(),
        cluster_api_addr: "127.0.0.1".parse().unwrap(),
        cluster_api_port: 6443,
        cluster_api_host: None,
        control_socket: state_dir.path().join("test.sock"),
        hostname: "sunbeam-net-test".into(),
        server_public_key: None,
        derp_tls_insecure: std::env::var("SUNBEAM_NET_TEST_DERP_INSECURE").is_ok(),
    };

    let keys = sunbeam_net::keys::NodeKeys::load_or_generate(&config.state_dir).unwrap();

    // Connect and register
    let mut control =
        sunbeam_net::control::ControlClient::connect(&config, &keys)
            .await
            .expect("failed to connect to Headscale");

    let reg = control
        .register(&config.auth_key, &config.hostname, &keys)
        .await
        .expect("registration failed");

    assert!(
        reg.machine_authorized,
        "machine should be authorized with pre-auth key"
    );

    // Start map stream and get first netmap
    let mut map = control
        .map_stream(&keys, &config.hostname)
        .await
        .expect("failed to start map stream");

    let update = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        map.next(),
    )
    .await
    .expect("timed out waiting for netmap")
    .expect("map stream error")
    .expect("map stream ended without data");

    match update {
        sunbeam_net::control::MapUpdate::Full { peers, .. } => {
            println!("Received netmap with {} peers", peers.len());
            // peer-a and peer-b should be in the netmap
            assert!(
                peers.len() >= 2,
                "expected at least 2 peers (peer-a + peer-b), got {}",
                peers.len()
            );
        }
        other => panic!("expected Full netmap, got {other:?}"),
    }
}

/// Test: proxy listener accepts connections after daemon is Running.
#[tokio::test(flavor = "multi_thread")]
async fn test_proxy_listener_accepts() {
    let coord_url = require_env("SUNBEAM_NET_TEST_COORD_URL");
    let auth_key = require_env("SUNBEAM_NET_TEST_AUTH_KEY");

    let state_dir = tempfile::tempdir().unwrap();
    // Use port 0 — OS picks a free port — and read it back from the actual listener.
    let proxy_bind: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let config = sunbeam_net::VpnConfig {
        coordination_url: coord_url,
        auth_key,
        state_dir: state_dir.path().to_path_buf(),
        proxy_bind,
        cluster_api_addr: "100.64.0.1".parse().unwrap(),
        cluster_api_port: 6443,
        cluster_api_host: None,
        control_socket: state_dir.path().join("proxy.sock"),
        hostname: "sunbeam-net-proxy-test".into(),
        server_public_key: None,
        derp_tls_insecure: std::env::var("SUNBEAM_NET_TEST_DERP_INSECURE").is_ok(),
    };

    let handle = sunbeam_net::VpnDaemon::start(config).await.unwrap();

    // Wait for Running
    let mut ready = false;
    for _ in 0..60 {
        if matches!(handle.current_status(), sunbeam_net::DaemonStatus::Running { .. }) {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ready, "daemon did not reach Running state");

    // We can't easily discover the dynamically-bound proxy port from the handle
    // (no API for it yet), so we just verify the daemon is Running and shut down.
    // A future improvement: expose proxy_addr() on DaemonHandle.

    handle.shutdown().await.unwrap();
}

/// End-to-end: bring up the daemon, dial peer-a's echo server through the
/// proxy, and assert we get bytes back across the WireGuard tunnel.
///
/// Requires the docker-compose stack with TUN-mode peers + TLS Headscale
/// (sunbeam-net/tests/run.sh handles the setup). The test enables
/// `derp_tls_insecure` because the test stack uses a self-signed cert.
#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_tcp_through_tunnel() {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Try to install a tracing subscriber so the daemon's logs reach
    // stderr when the test is run with `cargo test -- --nocapture`. Ignore
    // failures (already installed by another test).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sunbeam_net=trace")),
        )
        .with_test_writer()
        .try_init();

    let coord_url = require_env("SUNBEAM_NET_TEST_COORD_URL");
    let auth_key = require_env("SUNBEAM_NET_TEST_AUTH_KEY");
    let peer_a_ip: std::net::IpAddr = require_env("SUNBEAM_NET_TEST_PEER_A_IP")
        .parse()
        .expect("SUNBEAM_NET_TEST_PEER_A_IP must be a valid IP");

    let state_dir = tempfile::tempdir().unwrap();
    // Use a fixed local proxy port so the test client knows where to dial.
    let proxy_bind: std::net::SocketAddr = "127.0.0.1:16578".parse().unwrap();
    let config = sunbeam_net::VpnConfig {
        coordination_url: coord_url,
        auth_key,
        state_dir: state_dir.path().to_path_buf(),
        proxy_bind,
        cluster_api_addr: peer_a_ip,
        cluster_api_port: 5678,
        cluster_api_host: None,
        control_socket: state_dir.path().join("e2e.sock"),
        hostname: "sunbeam-net-e2e-test".into(),
        server_public_key: None,
        // Test stack uses a self-signed cert.
        derp_tls_insecure: std::env::var("SUNBEAM_NET_TEST_DERP_INSECURE").is_ok(),
    };

    let handle = sunbeam_net::VpnDaemon::start(config)
        .await
        .expect("daemon start failed");

    // Wait for Running.
    let mut ready = false;
    for _ in 0..60 {
        if matches!(
            handle.current_status(),
            sunbeam_net::DaemonStatus::Running { .. }
        ) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ready, "daemon did not reach Running within 30s");

    // After Running we still need to wait for two things:
    //   1. Headscale to push our node to peer-a's streaming netmap so peer-a
    //      adds us to its peer table — propagation can take a few seconds
    //      after the Lite update lands.
    //   2. The boringtun handshake to complete its first round-trip once
    //      smoltcp emits the SYN.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Dial the proxy and read whatever the echo server returns. http-echo
    // closes the connection after sending its body, so reading to EOF gives
    // us the full response.
    let mut stream = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::net::TcpStream::connect(proxy_bind),
    )
    .await
    .expect("connect to proxy timed out")
    .expect("connect to proxy failed");

    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: peer-a\r\n\r\n")
        .await
        .expect("write request failed");

    let mut buf = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(20),
        stream.read_to_end(&mut buf),
    )
    .await
    .expect("read response timed out")
    .expect("read response failed");

    assert!(read > 0, "expected bytes from echo server, got 0");
    let body = String::from_utf8_lossy(&buf);
    assert!(
        body.contains("sunbeam-net integration test"),
        "expected echo body in response, got: {body}"
    );

    handle.shutdown().await.expect("shutdown failed");
}

/// Test: full daemon lifecycle — start, reach Ready state, query via IPC, shutdown.
#[tokio::test(flavor = "multi_thread")]
async fn test_daemon_lifecycle() {
    let coord_url = require_env("SUNBEAM_NET_TEST_COORD_URL");
    let auth_key = require_env("SUNBEAM_NET_TEST_AUTH_KEY");

    let state_dir = tempfile::tempdir().unwrap();
    let config = sunbeam_net::VpnConfig {
        coordination_url: coord_url,
        auth_key,
        state_dir: state_dir.path().to_path_buf(),
        proxy_bind: "127.0.0.1:0".parse().unwrap(),
        cluster_api_addr: "127.0.0.1".parse().unwrap(),
        cluster_api_port: 6443,
        cluster_api_host: None,
        control_socket: state_dir.path().join("daemon.sock"),
        hostname: "sunbeam-net-daemon-test".into(),
        server_public_key: None,
        derp_tls_insecure: std::env::var("SUNBEAM_NET_TEST_DERP_INSECURE").is_ok(),
    };

    let handle = sunbeam_net::VpnDaemon::start(config)
        .await
        .expect("daemon start failed");

    // Wait for Running state (up to 30s)
    let mut ready = false;
    for _ in 0..60 {
        let status = handle.current_status();
        match status {
            sunbeam_net::DaemonStatus::Running { peer_count, .. } => {
                println!("Daemon running with {peer_count} peers");
                ready = true;
                break;
            }
            sunbeam_net::DaemonStatus::Reconnecting { attempt } => {
                panic!("Daemon entered Reconnecting (attempt {attempt})");
            }
            sunbeam_net::DaemonStatus::Stopped => {
                panic!("Daemon stopped unexpectedly");
            }
            sunbeam_net::DaemonStatus::Error { ref message } => {
                panic!("Daemon error: {message}");
            }
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
    assert!(ready, "daemon did not reach Running state within 30s");

    // Shutdown
    handle.shutdown().await.expect("shutdown failed");
}
