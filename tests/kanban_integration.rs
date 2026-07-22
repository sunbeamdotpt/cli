//! Integration tests for the kanban command layer against a real kanban server.
//!
//! Boots the full kanban stack via `sdk::testing::Kanban` — Postgres, NATS
//! (JetStream), OpenSearch, MinIO, and the sso-gateway stack — mints a
//! client-credentials token from the gateway, points the compiled `sunbeam`
//! binary at the server (`--url`, isolated `HOME` with a hand-written
//! `~/.sunbeam/config.json`), and drives the CLI end-to-end:
//!
//!   - project create / list / get
//!   - board create / list / get (+ column setup)
//!   - card create / list / move
//!   - name-resolution roundtrips (project/board/column/card by name, not ID)
//!
//! The suite skips cleanly when no Docker runtime is available. It also
//! skips when the stack fails to start: the kanban and sso-gateway images
//! live on ghcr.io (ghcr.io/sunbeamdotpt/kanban:latest and
//! ghcr.io/sunbeamdotpt/sso-gateway:latest) and the registry may require
//! authentication that is not available in every environment.

mod common;

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use sdk::reqwest;
use sdk::testing::Kanban;

const BIN: &str = env!("CARGO_BIN_EXE_sunbeam");

// ── CLI driving helpers ─────────────────────────────────────────────────────

/// Run `sunbeam kanban --url <url> -o json <args>` with an isolated HOME.
fn run_sunbeam(home: &Path, url: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.arg("kanban")
        .arg("--url")
        .arg(url)
        .arg("-o")
        .arg("json")
        .args(args)
        .env("HOME", home)
        // Keep cluster discovery hermetic; kanban commands never touch it.
        .env("KUBECONFIG", home.join("no-kubeconfig"));
    cmd.output().expect("spawn sunbeam binary")
}

/// Run a kanban command, assert success, and parse stdout as JSON.
fn json(home: &Path, url: &str, args: &[&str]) -> serde_json::Value {
    let out = run_sunbeam(home, url, args);
    assert!(
        out.status.success(),
        "sunbeam kanban {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON from sunbeam kanban {args:?}: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Mint an access token via the OAuth2 client-credentials grant against the
/// stack's sso-gateway. The provisioned application uses
/// `client_secret_post`, so credentials go in the form body.
async fn mint_token(gateway: &str, client_id: &str, client_secret: &str) -> Option<String> {
    let resp = reqwest::Client::new()
        .post(format!("{gateway}/oauth2/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", "permission:admin tenant:admin"),
        ])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("access_token")?.as_str().map(str::to_owned)
}

/// Write a minimal `~/.sunbeam/config.json`: one context whose domain keys
/// the cached-token entry the CLI's `auth::get_token` looks up.
fn write_config(home: &Path, domain: &str, token: &str) {
    let dir = home.join(".sunbeam");
    std::fs::create_dir_all(&dir).unwrap();
    let expires_at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let config = serde_json::json!({
        "current-context": "test",
        "contexts": { "test": { "domain": domain } },
        "auth": {
            domain: {
                "access_token": token,
                "expires_at": expires_at,
            }
        }
    });
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn kanban_cli_end_to_end() {
    if common::skip_without_docker("kanban_integration") {
        return;
    }

    let stack = match Kanban::new().start().await {
        Ok(stack) => stack,
        Err(e) => {
            eprintln!(
                "skipping kanban_integration: stack failed to start ({e}); \
                 the ghcr.io/sunbeamdotpt images may require registry authentication"
            );
            return;
        }
    };

    let token = mint_token(
        stack.sso_gateway_endpoint(),
        stack.client_id(),
        stack.client_secret(),
    )
    .await
    .expect("mint client-credentials token from sso-gateway");

    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "kanban.test", &token);
    let url = stack.endpoint().to_owned();
    let home = home.path();

    // ── Projects: create / list / get ────────────────────────────────────
    let project = json(
        home,
        &url,
        &[
            "project",
            "create",
            "--name",
            "Integration Test",
            "--prefix",
            "ITST",
        ],
    );
    assert_eq!(project["name"], "Integration Test");
    assert_eq!(project["prefix"], "ITST");

    let projects = json(home, &url, &["project", "list"]);
    assert!(
        projects
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "Integration Test"),
        "project list should contain the created project: {projects}"
    );

    // Name resolution: get by name, not ID.
    let got = json(home, &url, &["project", "get", "Integration Test"]);
    assert_eq!(got["id"], project["id"]);

    // ── Boards: create / list / get ──────────────────────────────────────
    let board = json(
        home,
        &url,
        &[
            "board",
            "create",
            "Integration Test",
            "--name",
            "Sprint Board",
        ],
    );
    assert_eq!(board["name"], "Sprint Board");

    let boards = json(home, &url, &["board", "list", "Integration Test"]);
    assert!(
        boards
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == "Sprint Board"),
        "board list should contain the created board: {boards}"
    );

    // Two columns so the card move has a destination (board name resolves).
    json(
        home,
        &url,
        &["board", "column", "add", "Sprint Board", "--title", "Todo"],
    );
    json(
        home,
        &url,
        &["board", "column", "add", "Sprint Board", "--title", "Done"],
    );

    let detail = json(home, &url, &["board", "get", "Sprint Board"]);
    let columns = detail["columns"].as_array().unwrap();
    let todo = columns
        .iter()
        .find(|c| c["title"] == "Todo")
        .expect("Todo column");
    let done = columns
        .iter()
        .find(|c| c["title"] == "Done")
        .expect("Done column");
    let todo_id = todo["id"].as_str().unwrap().to_string();
    let done_id = done["id"].as_str().unwrap().to_string();

    // ── Cards: create / list / move ──────────────────────────────────────
    let card = json(
        home,
        &url,
        &[
            "card",
            "create",
            "Sprint Board",
            "--column",
            &todo_id,
            "--title",
            "Card one",
        ],
    );
    assert_eq!(card["title"], "Card one");
    assert_eq!(card["column_id"], todo["id"]);
    let card_id = card["id"].as_str().unwrap().to_string();

    let cards = json(home, &url, &["card", "list", "Sprint Board"]);
    assert!(
        cards.as_array().unwrap().iter().any(|c| c["id"] == card_id),
        "card list should contain the created card: {cards}"
    );

    json(
        home,
        &url,
        &["card", "move", &card_id, "--column", &done_id],
    );
    let after_move = json(home, &url, &["card", "get", &card_id]);
    assert_eq!(after_move["column_id"], done["id"]);

    // Name-resolution roundtrip: fetch the card by title, not ID. Title
    // resolution goes through the search index (near-real-time), so allow
    // a short grace period for the new card to be indexed.
    let mut by_title = serde_json::Value::Null;
    for _ in 0..10 {
        let out = run_sunbeam(home, &url, &["card", "get", "Card one"]);
        if out.status.success() {
            by_title = serde_json::from_slice(&out.stdout).unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(by_title["id"], card_id, "card get by title should resolve");
}
