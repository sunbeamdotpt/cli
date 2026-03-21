#![cfg(feature = "integration")]

use sunbeam_sdk::client::{AuthMethod, HttpTransport, SunbeamClient};
use sunbeam_sdk::config::Context;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use reqwest::Method;

// ---------------------------------------------------------------------------
// 1. json() success — 200 + valid JSON
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/things"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": 42, "name": "widget"})),
        )
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let val: serde_json::Value = t
        .json(Method::GET, "/api/things", Option::<&()>::None, "fetch things")
        .await
        .unwrap();

    assert_eq!(val["id"], 42);
    assert_eq!(val["name"], "widget");
}

// ---------------------------------------------------------------------------
// 2. json() HTTP error — 500 returns SunbeamError::Network with ctx in msg
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fail"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal oops"))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .json::<serde_json::Value>(Method::GET, "/fail", Option::<&()>::None, "load stuff")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("load stuff"), "error should contain ctx: {msg}");
    assert!(msg.contains("500"), "error should contain status: {msg}");
}

// ---------------------------------------------------------------------------
// 3. json() parse error — 200 + invalid JSON
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bad-json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json {{{"))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .json::<serde_json::Value>(Method::GET, "/bad-json", Option::<&()>::None, "parse ctx")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("parse ctx"),
        "parse error should contain ctx: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 4. json_opt() success — 200 + JSON → Some(T)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_opt_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/item"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"found": true})),
        )
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let val: Option<serde_json::Value> = t
        .json_opt(Method::GET, "/item", Option::<&()>::None, "get item")
        .await
        .unwrap();

    assert!(val.is_some());
    assert_eq!(val.unwrap()["found"], true);
}

// ---------------------------------------------------------------------------
// 5. json_opt() 404 → None
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_opt_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let val: Option<serde_json::Value> = t
        .json_opt(Method::GET, "/missing", Option::<&()>::None, "lookup")
        .await
        .unwrap();

    assert!(val.is_none());
}

// ---------------------------------------------------------------------------
// 6. json_opt() server error — 500 → Err
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_opt_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/boom"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .json_opt::<serde_json::Value>(Method::GET, "/boom", Option::<&()>::None, "opt-fail")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("opt-fail"), "error should contain ctx: {msg}");
    assert!(msg.contains("500"), "error should contain status: {msg}");
}

// ---------------------------------------------------------------------------
// 7. send() success — 200 → Ok(())
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/action"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    t.send(Method::POST, "/action", Option::<&()>::None, "do action")
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// 8. send() error — 403 → Err
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/protected"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .send(Method::DELETE, "/protected", Option::<&()>::None, "delete thing")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("delete thing"), "error should contain ctx: {msg}");
    assert!(msg.contains("403"), "error should contain status: {msg}");
}

// ---------------------------------------------------------------------------
// 9. bytes() success — 200 + raw bytes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bytes_success() {
    let payload = b"binary-data-here";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/download"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.to_vec()))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let data = t.bytes(Method::GET, "/download", "fetch binary").await.unwrap();

    assert_eq!(data.as_ref(), payload);
}

// ---------------------------------------------------------------------------
// 10. bytes() error — 500
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bytes_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/download-fail"))
        .respond_with(ResponseTemplate::new(500).set_body_string("nope"))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .bytes(Method::GET, "/download-fail", "get bytes")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("get bytes"), "error should contain ctx: {msg}");
    assert!(msg.contains("500"), "error should contain status: {msg}");
}

// ---------------------------------------------------------------------------
// 11. request() with Bearer auth — verify Authorization header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auth-check"))
        .and(header("Authorization", "Bearer my-secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::Bearer("my-secret-token".into()));
    let val: serde_json::Value = t
        .json(Method::GET, "/auth-check", Option::<&()>::None, "bearer test")
        .await
        .unwrap();

    assert_eq!(val["ok"], true);
}

// ---------------------------------------------------------------------------
// 12. request() with Header auth — verify custom header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_header_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/vault"))
        .and(header("X-Vault-Token", "hvs.root-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"sealed": false})))
        .mount(&server)
        .await;

    let t = HttpTransport::new(
        &server.uri(),
        AuthMethod::Header {
            name: "X-Vault-Token",
            value: "hvs.root-token".into(),
        },
    );
    let val: serde_json::Value = t
        .json(Method::GET, "/vault", Option::<&()>::None, "header auth")
        .await
        .unwrap();

    assert_eq!(val["sealed"], false);
}

// ---------------------------------------------------------------------------
// 13. request() with Token auth — verify "token {pat}" format
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_token_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gitea"))
        .and(header("Authorization", "token pat-abc-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"user": "ci"})))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::Token("pat-abc-123".into()));
    let val: serde_json::Value = t
        .json(Method::GET, "/gitea", Option::<&()>::None, "token auth")
        .await
        .unwrap();

    assert_eq!(val["user"], "ci");
}

// ---------------------------------------------------------------------------
// 14. request() with None auth — no Authorization header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_no_auth() {
    let server = MockServer::start().await;

    // The mock only matches when there is NO Authorization header.
    // wiremock does not have a "header absent" matcher, so we just verify
    // the request succeeds (no auth header is fine) and inspect received
    // requests afterward.
    Mock::given(method("GET"))
        .and(path("/public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"public": true})))
        .expect(1)
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let val: serde_json::Value = t
        .json(Method::GET, "/public", Option::<&()>::None, "no auth")
        .await
        .unwrap();

    assert_eq!(val["public"], true);

    // Verify no Authorization header was sent.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert!(
        !reqs[0].headers.iter().any(|(k, _)| k == "authorization"),
        "Authorization header should not be present for AuthMethod::None"
    );
}

// ---------------------------------------------------------------------------
// 15. set_auth() — change auth, verify next request uses new auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_auth_changes_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/check"))
        .and(header("Authorization", "Bearer new-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(&server.uri(), AuthMethod::None);
    t.set_auth(AuthMethod::Bearer("new-tok".into()));

    let val: serde_json::Value = t
        .json(Method::GET, "/check", Option::<&()>::None, "after set_auth")
        .await
        .unwrap();

    assert_eq!(val["ok"], true);
}

// ---------------------------------------------------------------------------
// 16. URL construction — leading slash handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn url_construction_with_leading_slash() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a/b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"p": "ok"})))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);

    // With leading slash
    let val: serde_json::Value = t
        .json(Method::GET, "/a/b", Option::<&()>::None, "slash")
        .await
        .unwrap();
    assert_eq!(val["p"], "ok");
}

#[tokio::test]
async fn url_construction_without_leading_slash() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x/y"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"q": 1})))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);

    // Without leading slash
    let val: serde_json::Value = t
        .json(Method::GET, "x/y", Option::<&()>::None, "no-slash")
        .await
        .unwrap();
    assert_eq!(val["q"], 1);
}

#[tokio::test]
async fn url_construction_trailing_slash_stripped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"z": true})))
        .mount(&server)
        .await;

    // Base URL with trailing slash
    let t = HttpTransport::new(&format!("{}/", server.uri()), AuthMethod::None);
    let val: serde_json::Value = t
        .json(Method::GET, "/z", Option::<&()>::None, "trailing")
        .await
        .unwrap();
    assert_eq!(val["z"], true);
}

// ---------------------------------------------------------------------------
// 17. SunbeamClient::from_context() — domain, context accessors
// ---------------------------------------------------------------------------

#[test]
fn sunbeam_client_from_context() {
    let ctx = Context {
        domain: "test.sunbeam.dev".to_string(),
        kube_context: "k3s-test".to_string(),
        ssh_host: "root@10.0.0.1".to_string(),
        infra_dir: "/opt/infra".to_string(),
        acme_email: "ops@test.dev".to_string(),
    };

    let client = SunbeamClient::from_context(&ctx);

    assert_eq!(client.domain(), "test.sunbeam.dev");
    assert_eq!(client.context().domain, "test.sunbeam.dev");
    assert_eq!(client.context().kube_context, "k3s-test");
    assert_eq!(client.context().ssh_host, "root@10.0.0.1");
    assert_eq!(client.context().infra_dir, "/opt/infra");
    assert_eq!(client.context().acme_email, "ops@test.dev");
}

// ---------------------------------------------------------------------------
// Extra: json() with a request body (covers the Some(b) branch)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_with_request_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/create"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 99})),
        )
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let body = serde_json::json!({"name": "new-thing"});
    let val: serde_json::Value = t
        .json(Method::POST, "/create", Some(&body), "create thing")
        .await
        .unwrap();

    assert_eq!(val["id"], 99);
}

// ---------------------------------------------------------------------------
// Extra: json_opt() with a request body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_opt_with_request_body() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/update"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"updated": true})),
        )
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let body = serde_json::json!({"field": "value"});
    let val: Option<serde_json::Value> = t
        .json_opt(Method::PUT, "/update", Some(&body), "update thing")
        .await
        .unwrap();

    assert!(val.is_some());
    assert_eq!(val.unwrap()["updated"], true);
}

// ---------------------------------------------------------------------------
// Extra: send() with a request body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_with_request_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/submit"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let body = serde_json::json!({"payload": 123});
    t.send(Method::POST, "/submit", Some(&body), "submit data")
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Extra: json_opt() parse error — 200 + invalid JSON → Err
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_opt_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/bad-opt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<<<not json>>>"))
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .json_opt::<serde_json::Value>(Method::GET, "/bad-opt", Option::<&()>::None, "opt-parse")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("opt-parse"),
        "parse error should contain ctx: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Extra: error body text appears in Network error messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_body_text_in_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/err-body"))
        .respond_with(
            ResponseTemplate::new(422).set_body_string("validation failed: email required"),
        )
        .mount(&server)
        .await;

    let t = HttpTransport::new(&server.uri(), AuthMethod::None);
    let err = t
        .json::<serde_json::Value>(Method::GET, "/err-body", Option::<&()>::None, "validate")
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("422"), "should contain status: {msg}");
    assert!(
        msg.contains("validation failed"),
        "should contain body text: {msg}"
    );
}
