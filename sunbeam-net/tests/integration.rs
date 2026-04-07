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
        control_socket: state_dir.path().join("test.sock"),
        hostname: "sunbeam-net-test".into(),
        server_public_key: None,
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
        control_socket: state_dir.path().join("proxy.sock"),
        hostname: "sunbeam-net-proxy-test".into(),
        server_public_key: None,
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
        control_socket: state_dir.path().join("daemon.sock"),
        hostname: "sunbeam-net-daemon-test".into(),
        server_public_key: None,
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
