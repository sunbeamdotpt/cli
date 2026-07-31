//! Integration tests for the sso-gateway flows that `src/auth.rs` and
//! `src/users.rs` implement.
//!
//! These tests boot the real sso-gateway reference stack in Docker via
//! `sdk::testing::SsoGateway` and verify the contracts the CLI relies on:
//!
//! - OIDC discovery on the gateway's public endpoint (auth.rs login flow).
//! - The public device-authorization + token endpoints the CLI's RFC 8628
//!   login polls (auth.rs). The user-approval leg cannot be automated.
//! - Identity CRUD through `iam.v1.IdentityService`, exercised end to end by
//!   driving the `sunbeam` binary (`sunbeam user list/get/create/recover`)
//!   with `SUNBEAM_SSO_URL` pointed at the stack and a pre-minted token in an
//!   isolated HOME (users.rs).
//!
//! The whole suite skips cleanly when no Docker runtime is available, and
//! degrades to a skip notice when the gateway image cannot be pulled.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use sdk::auth::{AuthClient, v1 as iam};
use sdk::kanban::prelude::sunbeam_g2v;
use sdk::reqwest;
use sdk::testing::{SsoGateway, SsoGatewayHandle};

const BIN: &str = env!("CARGO_BIN_EXE_sunbeam");
const SYSTEM_BOOTSTRAP_CLIENT_ID: &str = "system-bootstrap-client";
const SYSTEM_TENANT_ULID: &str = "01HZY9JTKKHK3Y6XJJYHZ9Q5TV";
const BOOTSTRAP_SECRET: &str = "sunbeam-it-bootstrap-secret";

struct GatewayStack {
    endpoint: String,
    _gateway: SsoGatewayHandle,
    http: reqwest::Client,
}

/// Start the sso-gateway stack, returning None (with a skip notice) when the
/// image cannot be started/pulled in this environment.
async fn start_stack() -> Option<GatewayStack> {
    let tag = std::env::var("SSO_GATEWAY_IMAGE_TAG").unwrap_or_else(|_| "v2026.07.22".to_string());
    let gateway = match SsoGateway::new()
        .with_image(SsoGateway::DEFAULT_IMAGE_NAME, &tag)
        .with_env("SYSTEM_BOOTSTRAP_CLIENT_SECRET", BOOTSTRAP_SECRET)
        .start()
        .await
    {
        Ok(g) => g,
        Err(e) => {
            eprintln!(
                "skipping sso_gateway_integration: stack failed to start ({e}); \
                 the ghcr.io/sunbeamdotpt image may require registry authentication"
            );
            return None;
        }
    };
    let endpoint = gateway.endpoint().to_string();
    Some(GatewayStack {
        endpoint,
        _gateway: gateway,
        http: reqwest::Client::new(),
    })
}

/// Exchange the system bootstrap client credentials for an access token with
/// the requested scope (the gateway expects HTTP Basic auth here).
async fn fetch_bootstrap_token(gateway_url: &str, scope: &str) -> Result<String, String> {
    // Hydra may take a moment longer than the gateway readiness probe.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let resp: serde_json::Value = reqwest::Client::new()
        .post(format!("{gateway_url}/oauth2/token"))
        .basic_auth(SYSTEM_BOOTSTRAP_CLIENT_ID, Some(BOOTSTRAP_SECRET))
        .form(&[("grant_type", "client_credentials"), ("scope", scope)])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    resp.get("access_token")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "token response has no access_token".to_string())
}

fn admin_client(gateway_url: &str, token: &str) -> Result<AuthClient, String> {
    let g2v = AuthClient::builder(gateway_url)
        .auth(sunbeam_g2v::client::BearerToken::new(token))
        .build()
        .map_err(|e| e.to_string())?;
    let base_uri = gateway_url
        .parse()
        .map_err(|_| format!("invalid sso-gateway URL: {gateway_url}"))?;
    AuthClient::new(g2v, base_uri).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Discovery + public device endpoints (auth.rs contract)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oidc_discovery_advertises_device_flow() {
    if common::skip_without_docker("sso_gateway") {
        return;
    }
    let Some(stack) = start_stack().await else {
        return;
    };

    let doc: serde_json::Value = stack
        .http
        .get(format!(
            "{}/.well-known/openid-configuration",
            stack.endpoint
        ))
        .send()
        .await
        .expect("discovery request")
        .json()
        .await
        .expect("discovery json");

    let token_endpoint = doc["token_endpoint"].as_str().expect("token_endpoint");
    assert!(
        token_endpoint.ends_with("/oauth2/token"),
        "{token_endpoint}"
    );
    let device_endpoint = doc["device_authorization_endpoint"]
        .as_str()
        .expect("device_authorization_endpoint");
    assert!(
        device_endpoint.ends_with("/oauth2/device/auth"),
        "{device_endpoint}"
    );
}

/// Register a public device-flow application through the IAM API, then drive
/// the same public endpoints auth.rs uses: device code issuance and the
/// `authorization_pending` token poll.
///
/// NOTE: requires gateway >= v2026.07.22, where the oauth2 proxy relays
/// Hydra 4xx bodies verbatim so the token poll returns a proper RFC 8628
/// `authorization_pending` (earlier images collapsed it into a bare
/// `{"error":"server_error"}`). The user-approval leg cannot be automated;
/// the CLI's full polling logic is covered by wiremock unit tests in
/// src/auth.rs.
#[tokio::test]
async fn device_code_issuance_and_pending_poll() {
    if common::skip_without_docker("sso_gateway") {
        return;
    }
    let Some(stack) = start_stack().await else {
        return;
    };

    let token = fetch_bootstrap_token(&stack.endpoint, "application:admin")
        .await
        .expect("bootstrap token");
    let client = admin_client(&stack.endpoint, &token)
        .expect("admin client")
        .with_tenant(SYSTEM_TENANT_ULID);

    let app = client
        .application()
        .create_application(iam::CreateApplicationRequest {
            name: "sunbeam-cli-it".to_string(),
            redirect_uris: vec!["http://localhost:9876/callback".to_string()],
            grant_types: vec!["urn:ietf:params:oauth:grant-type:device_code".to_string()],
            response_types: vec![],
            scope: vec!["openid".to_string()],
            token_endpoint_auth_method: "none".to_string(),
            ..Default::default()
        })
        .await
        .expect("CreateApplication should succeed for public device client")
        .into_owned();

    // Mirror auth.rs request_device_code(): form POST with client_id + scope.
    let resp = stack
        .http
        .post(format!("{}/oauth2/device/auth", stack.endpoint))
        .form(&[("client_id", app.id.as_str()), ("scope", "openid")])
        .send()
        .await
        .expect("device auth request");
    let status = resp.status();
    let body = resp.text().await.expect("device auth body");
    assert!(
        status.is_success(),
        "device authorization failed: {status} {body}"
    );
    let device: serde_json::Value = serde_json::from_str(&body).expect("device auth json");

    let device_code = device["device_code"].as_str().expect("device_code");
    assert!(!device_code.is_empty());
    assert!(device["user_code"].as_str().expect("user_code").len() >= 8);

    // Mirror auth.rs poll_device_token(): before the user approves, the token
    // endpoint must answer 400 with error=authorization_pending.
    let resp = stack
        .http
        .post(format!("{}/oauth2/token", stack.endpoint))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", app.id.as_str()),
        ])
        .send()
        .await
        .expect("token poll request");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let err: serde_json::Value = resp.json().await.expect("error json");
    assert_eq!(
        err["error"], "authorization_pending",
        "token poll error body: {err}"
    );
}

// ---------------------------------------------------------------------------
// Identity CRUD through the CLI binary (users.rs contract)
// ---------------------------------------------------------------------------

/// Mint a token with identity scopes and wrap it in an [`AuthClient`] bound
/// to the system tenant. The token itself is what the CLI binary later uses;
/// the client provisions prerequisites (identity schema) up front.
///
/// Bootstrap tokens cannot grant `identity:*` scopes, so this goes through a
/// purpose-built client credential.
async fn identity_client(gateway_url: &str) -> Result<(AuthClient, String), String> {
    let app_token = fetch_bootstrap_token(gateway_url, "application:admin").await?;
    let client = admin_client(gateway_url, &app_token)?.with_tenant(SYSTEM_TENANT_ULID);

    let cred = client
        .client_credentials()
        .create_client_credential(iam::CreateClientCredentialRequest {
            name: "sunbeam-users-it".to_string(),
            scope: vec!["identity:admin".to_string(), "identity:read".to_string()],
            token_endpoint_auth_method: "client_secret_post".to_string(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("CreateClientCredential failed: {e}"))?
        .into_owned();

    let resp: serde_json::Value = reqwest::Client::new()
        .post(format!("{gateway_url}/oauth2/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", cred.id.as_str()),
            ("client_secret", cred.client_secret.as_str()),
            ("scope", "identity:admin identity:read"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let token = resp
        .get("access_token")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "token response has no access_token".to_string())?;

    Ok((
        admin_client(gateway_url, &token)?.with_tenant(SYSTEM_TENANT_ULID),
        token,
    ))
}

/// Ensure the tenant has an identity schema the CLI can create identities
/// under. Returns the schema id to pass via `--schema`.
async fn ensure_identity_schema(client: &AuthClient) -> Result<String, String> {
    let schemas = client
        .identity()
        .list_identity_schemas(iam::ListIdentitySchemasRequest::default())
        .await
        .map_err(|e| format!("ListIdentitySchemas failed: {e}"))?
        .into_owned();
    if let Some(s) = schemas.schemas.first() {
        return Ok(s.schema_id.clone());
    }

    let schema_json: sdk::kanban::prelude::buffa_types::google::protobuf::Struct =
        serde_json::from_value(serde_json::json!({
            "$id": "https://schemas.sunbeam.pt/it-identity.json",
            "type": "object",
            "properties": {
                "traits": {
                    "type": "object",
                    "required": ["email"],
                    "properties": {
                        "email": { "type": "string", "format": "email" },
                        "name": {
                            "type": "object",
                            "properties": {
                                "first": { "type": "string" },
                                "last": { "type": "string" }
                            }
                        }
                    }
                }
            }
        }))
        .map_err(|e| format!("schema json: {e}"))?;

    let created = client
        .identity()
        .create_identity_schema(iam::CreateIdentitySchemaRequest {
            schema_id: "default".to_string(),
            schema_json: sdk::kanban::prelude::buffa::MessageField::some(schema_json),
            is_default: true,
            ..Default::default()
        })
        .await
        .map_err(|e| format!("CreateIdentitySchema failed: {e}"))?
        .into_owned();
    Ok(created.schema_id)
}

/// Run `sunbeam <args>` with an isolated HOME and SUNBEAM_SSO_URL override.
fn run_sunbeam(home: &Path, gateway_url: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env("HOME", home)
        .env("SUNBEAM_SSO_URL", gateway_url)
        // Keep cluster discovery hermetic; user commands never touch it.
        .env("KUBECONFIG", home.join("no-kubeconfig"));
    cmd.output().expect("spawn sunbeam binary")
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

/// The sdk's `SsoGateway` Kratos config does not enable the recovery courier,
/// so the Kratos admin recovery endpoint answers 404 "disabled by system
/// administrator". The gateway's `CreateRecoveryLink` surfaces that to the
/// CLI. Detect it so the CRUD test can tolerate either stack behavior.
fn recovery_disabled(stderr: &str) -> bool {
    stderr.contains("disabled by system administrator")
}

#[tokio::test]
async fn cli_user_crud_end_to_end() {
    if common::skip_without_docker("sso_gateway") {
        return;
    }
    let Some(stack) = start_stack().await else {
        return;
    };

    let (id_client, token) = identity_client(&stack.endpoint)
        .await
        .expect("identity-scoped client");
    let schema = ensure_identity_schema(&id_client)
        .await
        .expect("identity schema");

    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "sso.test", &token);
    let home = home.path();
    let url = stack.endpoint.clone();

    // user list (empty directory is fine).
    let out = run_sunbeam(home, &url, &["user", "list"]);
    assert!(
        out.status.success(),
        "user list failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // user create → identity created, then a recovery link + token is printed.
    // In this stack the recovery step fails (Kratos recovery courier disabled);
    // the identity itself is still created, which is what we assert on.
    let out = run_sunbeam(
        home,
        &url,
        &[
            "user",
            "create",
            "it-user@sunbeam.test",
            "--name",
            "IT User",
            "--schema",
            &schema,
        ],
    );
    if out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("recovery") || stdout.contains("flow="),
            "expected a recovery link in output: {stdout}"
        );
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            recovery_disabled(&stderr),
            "user create failed unexpectedly:\n{stderr}"
        );
    }

    // user list --search finds the new identity.
    let out = run_sunbeam(home, &url, &["user", "list", "--search", "it-user"]);
    assert!(
        out.status.success(),
        "user list --search failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("it-user@sunbeam.test"),
        "identity missing from list: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // user get by email renders the identity JSON.
    let out = run_sunbeam(home, &url, &["user", "get", "it-user@sunbeam.test"]);
    assert!(
        out.status.success(),
        "user get failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON from user get: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert!(
        got["traits"]["email"]
            .as_str()
            .is_some_and(|e| e.eq_ignore_ascii_case("it-user@sunbeam.test")),
        "unexpected identity: {got}"
    );

    // user recover prints a fresh recovery link (or hits the stack's disabled
    // recovery courier — either documents the CLI↔gateway contract).
    let out = run_sunbeam(home, &url, &["user", "recover", "it-user@sunbeam.test"]);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            recovery_disabled(&stderr),
            "user recover failed unexpectedly:\n{stderr}"
        );
    }

    // user delete with "y" piped to stdin confirms and deletes.
    let mut cmd = Command::new(BIN);
    let out = cmd
        .args(["user", "delete", "it-user@sunbeam.test"])
        .env("HOME", home)
        .env("SUNBEAM_SSO_URL", &url)
        .env("KUBECONFIG", home.join("no-kubeconfig"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(b"y\n")
                .unwrap();
            child.wait_with_output()
        })
        .expect("run user delete");
    assert!(
        out.status.success(),
        "user delete failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Deleted identity no longer resolves.
    let out = run_sunbeam(home, &url, &["user", "get", "it-user@sunbeam.test"]);
    assert!(
        !out.status.success(),
        "user get should fail after delete:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
