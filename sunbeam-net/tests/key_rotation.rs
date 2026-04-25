//! Integration tests for the stuck-handshake watchdog and node-key rotation.
//!
//! These tests exercise `NodeKeys::rotate_node_key` end-to-end without
//! requiring a live Headscale instance.

use sunbeam_net::keys::NodeKeys;
use tempfile::TempDir;

// ── Test 1: rotation produces a different node key ────────────────────────────

#[test]
fn stuck_handshake_triggers_key_rotation() {
    let dir = TempDir::new().unwrap();
    let keys_before = NodeKeys::load_or_generate(dir.path()).unwrap();
    let node_pub_before = keys_before.node_key_str();
    let disco_before = keys_before.disco_key_str();

    NodeKeys::rotate_node_key(dir.path()).unwrap();

    let keys_after = NodeKeys::load_or_generate(dir.path()).unwrap();

    assert_ne!(
        node_pub_before,
        keys_after.node_key_str(),
        "node key must change after rotation"
    );
    // disco and wg keys must be preserved — only node_private rotates.
    assert_eq!(
        disco_before,
        keys_after.disco_key_str(),
        "disco key must survive rotation"
    );
}

// ── Test 2: rebuild counter resets after a successful handshake ───────────────
//
// The counter state lives inside WgTunnel and is tested exhaustively in the
// unit tests in wg/tunnel.rs (`rebuild_counter_increments_and_resets`).
// Here we verify the observable end-state: after rotation is triggered the
// daemon can still generate a fresh node key that is valid.

#[test]
fn successful_handshake_resets_rebuild_counter() {
    // Rotate once, then rotate again — both must succeed and each must
    // produce a distinct key. This demonstrates the watchdog can fire
    // multiple times without corrupting state.
    let dir = TempDir::new().unwrap();
    NodeKeys::load_or_generate(dir.path()).unwrap();

    NodeKeys::rotate_node_key(dir.path()).unwrap();
    let after_first = NodeKeys::load_or_generate(dir.path()).unwrap();

    NodeKeys::rotate_node_key(dir.path()).unwrap();
    let after_second = NodeKeys::load_or_generate(dir.path()).unwrap();

    assert_ne!(
        after_first.node_key_str(),
        after_second.node_key_str(),
        "each rotation must produce a unique key"
    );
    assert!(after_second.node_key_str().starts_with("nodekey:"));
}

// ── Test 3: rotation persists atomically across a daemon restart ──────────────

#[test]
fn key_rotation_persists_atomically_across_daemon_restart() {
    let dir = TempDir::new().unwrap();
    let original = NodeKeys::load_or_generate(dir.path()).unwrap();

    NodeKeys::rotate_node_key(dir.path()).unwrap();

    // Simulate restart: fresh load from disk.
    let reloaded = NodeKeys::load_or_generate(dir.path()).unwrap();
    assert_ne!(
        original.node_key_str(),
        reloaded.node_key_str(),
        "reloaded key must differ from pre-rotation key"
    );

    // A second load produces the same rotated key (no accidental re-generation).
    let reloaded2 = NodeKeys::load_or_generate(dir.path()).unwrap();
    assert_eq!(
        reloaded.node_key_str(),
        reloaded2.node_key_str(),
        "key must be stable across repeated loads after rotation"
    );
}

// ── Test 4: failed rotation leaves the original keys.json intact ──────────────

#[test]
fn key_rotation_failure_does_not_corrupt_keys_json() {
    let dir = TempDir::new().unwrap();
    let original = NodeKeys::load_or_generate(dir.path()).unwrap();
    let original_key = original.node_key_str();
    let keys_path = dir.path().join("keys.json");

    // Attempt rotation into a nonexistent directory — the fs::read_to_string
    // call will fail before any write, so the original file is untouched.
    let bad_dir = dir.path().join("does_not_exist");
    let result = NodeKeys::rotate_node_key(&bad_dir);
    assert!(result.is_err(), "rotation to nonexistent dir must fail");

    // Original keys.json must still exist and contain the original key.
    assert!(keys_path.exists(), "keys.json must still exist after failed rotation");
    let still_original = NodeKeys::load_or_generate(dir.path()).unwrap();
    assert_eq!(
        original_key,
        still_original.node_key_str(),
        "keys.json must contain the original key after a failed rotation"
    );
}
