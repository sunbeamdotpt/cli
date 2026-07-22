//! Integration test for `sdk::vpn::cmds::cmd_vpn_create_key` against a real
//! Headscale container.
//!
//! Headscale pre-auth keys are minted through its REST API using an API key
//! that must be bootstrapped via the headscale CLI. The test execs into the
//! container to create a user and an API key, points the SDK's active
//! context at the container, and then calls the real SDK command.
//!
//! Note: the SDK's active context is a process-wide `OnceLock`, so this
//! suite keeps a single test that performs every phase in order.
//!
//! The suite skips cleanly when no Docker runtime is available.

mod common;

use sdk::config::Context;
use sdk::testing::Headscale;
use testcontainers::core::ExecCommand;

/// Run a headscale CLI command inside the container and return stdout.
async fn headscale_cli(
    container: &testcontainers::ContainerAsync<testcontainers::GenericImage>,
    args: &[&str],
) -> String {
    let mut cmd = vec!["headscale"];
    cmd.extend_from_slice(args);
    let mut exec = container
        .exec(ExecCommand::new(cmd))
        .await
        .expect("exec into headscale container");
    let out = exec.stdout_to_vec().await.expect("read exec stdout");
    let code = exec.exit_code().await.expect("exec exit code");
    assert_eq!(code, Some(0), "headscale {args:?} failed");
    String::from_utf8(out)
        .expect("utf8 stdout")
        .trim()
        .to_string()
}

#[tokio::test]
async fn create_preauth_key_against_real_headscale() {
    if common::skip_without_docker("headscale") {
        return;
    }

    let container = Headscale::new()
        .publish_ports()
        .start()
        .await
        .expect("headscale container should start");
    let url = Headscale::url(&container)
        .await
        .expect("headscale url should resolve");

    // Bootstrap: create the user the CLI will mint a key for, and an API key
    // to authenticate the REST calls with.
    headscale_cli(&container, &["users", "create", "sunbeam"]).await;
    let api_key = headscale_cli(&container, &["apikeys", "create", "--expiration", "1h"]).await;
    assert!(!api_key.is_empty(), "apikeys create returned no key");

    // Point the SDK's active context at the container (process-wide, set once).
    sdk::config::set_active_context(Context {
        vpn_url: url.clone(),
        vpn_api_key: api_key,
        ..Default::default()
    });

    // The real SDK command: resolves the user name to a numeric ID via
    // GET /api/v1/user, then POSTs /api/v1/preauthkey.
    let key = sdk::vpn::cmds::cmd_vpn_create_key("sunbeam", None, true, false, "24h")
        .await
        .expect("cmd_vpn_create_key should mint a key");
    assert!(!key.is_empty(), "empty pre-auth key");

    // The minted key must be listed by headscale.
    let listed = headscale_cli(&container, &["preauthkeys", "list", "-o", "json"]).await;
    assert!(
        listed.contains(&key[..key.len().min(12)]),
        "minted key not found in preauthkeys list:\n{listed}"
    );

    // A name that does not resolve to a user must surface an error.
    let err = sdk::vpn::cmds::cmd_vpn_create_key("ghost-user", None, false, false, "1h")
        .await
        .expect_err("unknown user should fail");
    assert!(
        err.to_string().contains("ghost-user") || err.to_string().contains("not found"),
        "unexpected error: {err}"
    );
}
