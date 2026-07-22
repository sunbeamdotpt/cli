//! Integration tests for `sdk::openbao::BaoClient` against a real OpenBao
//! dev-mode container.
//!
//! Dev mode auto-initializes and auto-unseals the server with a known root
//! token, so these tests exercise the client surface against a live API:
//! seal status, init/unseal error paths, KV v2 CRUD, engine management,
//! and the transit engine via the generic read/write helpers.
//!
//! The whole suite skips cleanly when no Docker runtime is available.

mod common;

use std::collections::HashMap;

use sdk::openbao::BaoClient;
use sdk::testing::OpenBao;

/// Start a dev-mode OpenBao container and return (container, authed client).
async fn start_bao() -> (
    testcontainers::ContainerAsync<testcontainers::GenericImage>,
    BaoClient,
) {
    let container = OpenBao::new()
        .publish_ports()
        .start()
        .await
        .expect("openbao container should start");
    let url = OpenBao::url(&container)
        .await
        .expect("openbao url should resolve");
    let client = BaoClient::with_token(&url, OpenBao::DEFAULT_ROOT_TOKEN);
    (container, client)
}

fn map_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn seal_status_initialized_and_unsealed() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    let status = client.seal_status().await.expect("seal_status");
    assert!(status.initialized, "dev-mode server is initialized");
    assert!(!status.sealed, "dev-mode server is unsealed");
}

#[tokio::test]
async fn init_on_initialized_server_fails() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    let err = client
        .init(5, 3)
        .await
        .expect_err("init on an initialized server must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("init") || msg.contains("initialized"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn unseal_on_unsealed_server_fails() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    // Dev mode is already unsealed; OpenBao rejects unseal attempts against a
    // server that is not sealed (400), which surfaces as an error here.
    let err = client
        .unseal("bogus-key")
        .await
        .expect_err("unseal on an unsealed server must fail");
    assert!(
        err.to_string().contains("unseal"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn kv_v2_full_crud_cycle() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    // Dev mode mounts a KV v2 engine at "secret".
    let mount = "secret";
    let path = "sunbeam-it/crud";

    // Create.
    client
        .kv_put(
            mount,
            path,
            &map_of(&[("username", "alice"), ("password", "s3cret")]),
        )
        .await
        .expect("kv_put");

    // Read back.
    let got = client.kv_get(mount, path).await.expect("kv_get");
    let got = got.expect("secret should exist after put");
    assert_eq!(got.get("username").map(String::as_str), Some("alice"));
    assert_eq!(got.get("password").map(String::as_str), Some("s3cret"));

    // Single-field read.
    let field = client
        .kv_get_field(mount, path, "username")
        .await
        .expect("kv_get_field");
    assert_eq!(field, "alice");

    // Patch merges without dropping untouched keys.
    client
        .kv_patch(mount, path, &map_of(&[("password", "hunter2")]))
        .await
        .expect("kv_patch");
    let got = client
        .kv_get(mount, path)
        .await
        .expect("kv_get after patch")
        .expect("secret should still exist");
    assert_eq!(got.get("username").map(String::as_str), Some("alice"));
    assert_eq!(got.get("password").map(String::as_str), Some("hunter2"));

    // List the parent prefix via the generic LIST helper.
    let list = client
        .list("secret/metadata/sunbeam-it")
        .await
        .expect("list")
        .expect("metadata prefix should exist");
    let keys = list["data"]["keys"].as_array().expect("keys array");
    assert!(
        keys.iter().any(|k| k.as_str() == Some("crud")),
        "expected 'crud' in {keys:?}"
    );

    // Delete; a subsequent read fails because vaultrs reports the 404 for a
    // soft-deleted KV v2 secret as a generic request error (not the "404"
    // string kv_get matches on). The secret no longer shows up in data reads.
    client.kv_delete(mount, path).await.expect("kv_delete");
    let result = client.kv_get(mount, path).await;
    assert!(
        result.is_err() || result.ok().flatten().is_none(),
        "secret should be unreadable after delete"
    );

    // Deleting a missing secret is tolerated (idempotent).
    client
        .kv_delete(mount, path)
        .await
        .expect("delete of missing secret should be a no-op");
}

#[tokio::test]
async fn kv_get_missing_returns_none() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    let missing = client
        .kv_get("secret", "sunbeam-it/definitely-not-there")
        .await
        .expect("kv_get of missing path");
    assert!(missing.is_none());

    // kv_get_field on a missing secret yields an empty string, not an error.
    let field = client
        .kv_get_field("secret", "sunbeam-it/definitely-not-there", "whatever")
        .await
        .expect("kv_get_field of missing path");
    assert!(field.is_empty());
}

#[tokio::test]
async fn enable_secrets_engine_is_idempotent() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    client
        .enable_secrets_engine("sunbeam-kv", "kv-v2")
        .await
        .expect("enable kv-v2");
    // Second enable hits the "already in use" path and must still succeed.
    client
        .enable_secrets_engine("sunbeam-kv", "kv-v2")
        .await
        .expect("enable kv-v2 again (idempotent)");

    // The new mount is usable through the KV helpers.
    client
        .kv_put("sunbeam-kv", "ping", &map_of(&[("pong", "1")]))
        .await
        .expect("kv_put on new mount");
    let got = client
        .kv_get_field("sunbeam-kv", "ping", "pong")
        .await
        .expect("kv_get_field on new mount");
    assert_eq!(got, "1");
}

#[tokio::test]
async fn transit_engine_encrypt_decrypt_roundtrip() {
    if common::skip_without_docker("openbao") {
        return;
    }
    use base64::Engine;
    let (_container, client) = start_bao().await;

    client
        .enable_secrets_engine("transit", "transit")
        .await
        .expect("enable transit");

    // Create a named key.
    client
        .write(
            "transit/keys/sunbeam-it",
            &serde_json::json!({"type": "aes256-gcm96"}),
        )
        .await
        .expect("create transit key");

    // The key is visible through the generic read helper.
    let key_info = client
        .read("transit/keys/sunbeam-it")
        .await
        .expect("read transit key")
        .expect("transit key should exist");
    assert_eq!(key_info["data"]["type"], "aes256-gcm96");

    // Encrypt.
    let plaintext_b64 =
        base64::engine::general_purpose::STANDARD.encode("sunbeam integration test");
    let encrypted = client
        .write(
            "transit/encrypt/sunbeam-it",
            &serde_json::json!({"plaintext": plaintext_b64}),
        )
        .await
        .expect("encrypt");
    let ciphertext = encrypted["data"]["ciphertext"]
        .as_str()
        .expect("ciphertext");
    assert!(
        ciphertext.starts_with("vault:v"),
        "unexpected ciphertext: {ciphertext}"
    );

    // Decrypt and verify the roundtrip.
    let decrypted = client
        .write(
            "transit/decrypt/sunbeam-it",
            &serde_json::json!({"ciphertext": ciphertext}),
        )
        .await
        .expect("decrypt");
    let decoded_b64 = decrypted["data"]["plaintext"].as_str().expect("plaintext");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(decoded_b64)
        .expect("base64 decode");
    assert_eq!(decoded, b"sunbeam integration test");

    // Reading a nonexistent path yields None.
    let missing = client
        .read("transit/keys/no-such-key")
        .await
        .expect("read missing key");
    assert!(missing.is_none());
}

#[tokio::test]
async fn auth_enable_and_policy_write() {
    if common::skip_without_docker("openbao") {
        return;
    }
    let (_container, client) = start_bao().await;

    client
        .auth_enable("sunbeam-userpass", "userpass")
        .await
        .expect("enable userpass");
    // Idempotent on repeat.
    client
        .auth_enable("sunbeam-userpass", "userpass")
        .await
        .expect("enable userpass again (idempotent)");

    let policy = r#"path "secret/data/sunbeam-it/*" { capabilities = ["read"] }"#;
    client
        .write_policy("sunbeam-it-read", policy)
        .await
        .expect("write policy");

    let read_back = client
        .read("sys/policy/sunbeam-it-read")
        .await
        .expect("read policy")
        .expect("policy should exist");
    let rules = read_back["rules"].as_str().expect("policy rules");
    assert!(rules.contains("secret/data/sunbeam-it/*"), "rules: {rules}");
}
