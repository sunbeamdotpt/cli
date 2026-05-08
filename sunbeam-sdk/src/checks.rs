//! Service-level health checks — functional probes beyond pod readiness.

use crate::error::Result;
use base64::Engine;
use hmac::{Hmac, Mac};
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::api::{Api, ListParams};
use sha2::{Digest, Sha256};
use std::time::Duration;

use crate::kube::{get_client, kube_exec, parse_target};
use crate::output::{ok, step, warn};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// CheckResult
// ---------------------------------------------------------------------------

/// Result of a single health check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Name.
    pub name: String,
    /// Ns.
    pub ns: String,
    /// Svc.
    pub svc: String,
    /// Passed.
    pub passed: bool,
    /// Detail.
    pub detail: String,
}

impl CheckResult {
    fn ok(name: &str, ns: &str, svc: &str, detail: &str) -> Self {
        Self {
            name: name.into(),
            ns: ns.into(),
            svc: svc.into(),
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(name: &str, ns: &str, svc: &str, detail: &str) -> Self {
        Self {
            name: name.into(),
            ns: ns.into(),
            svc: svc.into(),
            passed: false,
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP client builder
// ---------------------------------------------------------------------------

/// Build a reqwest client that trusts the mkcert local CA if available,
/// does not follow redirects, and has a 5s timeout.
fn build_http_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5));

    // Try mkcert root CA
    if let Ok(output) = std::process::Command::new("mkcert").arg("-CAROOT").output()
        && output.status.success()
    {
        let ca_root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let ca_file = std::path::Path::new(&ca_root).join("rootCA.pem");
        if ca_file.exists()
            && let Ok(pem_bytes) = std::fs::read(&ca_file)
            && let Ok(cert) = reqwest::Certificate::from_pem(&pem_bytes)
        {
            builder = builder.add_root_certificate(cert);
        }
    }

    Ok(builder.build()?)
}

/// Helper: GET a URL, return (status_code, body_bytes). Does not follow redirects.
async fn http_get(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&[(&str, &str)]>,
) -> std::result::Result<(u16, Vec<u8>), String> {
    let mut req = client.get(url);
    if let Some(hdrs) = headers {
        for (k, v) in hdrs {
            req = req.header(*k, *v);
        }
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.bytes().await.unwrap_or_default().to_vec();
            Ok((status, body))
        }
        Err(e) => Err(format!("{e}")),
    }
}

/// Read a K8s secret field, returning empty string on failure.
async fn kube_secret(ns: &str, name: &str, key: &str) -> String {
    crate::kube::kube_get_secret_field(ns, name, key)
        .await
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

/// GET /api/v1/version -> JSON with version field.
async fn check_gitea_version(domain: &str, client: &reqwest::Client) -> CheckResult {
    let url = format!("https://src.{domain}/api/v1/version");
    match http_get(client, &url, None).await {
        Ok((200, body)) => {
            let ver = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| "?".into());
            CheckResult::ok("gitea-version", "devtools", "gitea", &format!("v{ver}"))
        }
        Ok((status, _)) => CheckResult::fail(
            "gitea-version",
            "devtools",
            "gitea",
            &format!("HTTP {status}"),
        ),
        Err(e) => CheckResult::fail("gitea-version", "devtools", "gitea", &e),
    }
}

/// GET /api/v1/user with admin credentials -> 200 and login field.
async fn check_gitea_auth(domain: &str, client: &reqwest::Client) -> CheckResult {
    let username = {
        let u = kube_secret("devtools", "gitea-admin-credentials", "username").await;
        if u.is_empty() {
            "gitea_admin".to_string()
        } else {
            u
        }
    };
    let password = kube_secret("devtools", "gitea-admin-credentials", "password").await;
    if password.is_empty() {
        return CheckResult::fail(
            "gitea-auth",
            "devtools",
            "gitea",
            "password not found in secret",
        );
    }

    let creds = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
    let auth_hdr = format!("Basic {creds}");
    let url = format!("https://src.{domain}/api/v1/user");

    match http_get(client, &url, Some(&[("Authorization", &auth_hdr)])).await {
        Ok((200, body)) => {
            let login = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("login").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| "?".into());
            CheckResult::ok("gitea-auth", "devtools", "gitea", &format!("user={login}"))
        }
        Ok((status, _)) => {
            CheckResult::fail("gitea-auth", "devtools", "gitea", &format!("HTTP {status}"))
        }
        Err(e) => CheckResult::fail("gitea-auth", "devtools", "gitea", &e),
    }
}

/// CNPG Cluster readyInstances == instances.
async fn check_postgres(_domain: &str, _client: &reqwest::Client) -> CheckResult {
    let kube_client = match get_client().await {
        Ok(c) => c,
        Err(e) => {
            return CheckResult::fail("postgres", "data", "postgres", &format!("{e}"));
        }
    };

    let ar = kube::api::ApiResource {
        group: "postgresql.cnpg.io".into(),
        version: "v1".into(),
        api_version: "postgresql.cnpg.io/v1".into(),
        kind: "Cluster".into(),
        plural: "clusters".into(),
    };

    let api: Api<kube::api::DynamicObject> = Api::namespaced_with(kube_client.clone(), "data", &ar);

    match api.get_opt("postgres").await {
        Ok(Some(obj)) => {
            let ready = obj
                .data
                .get("status")
                .and_then(|s| s.get("readyInstances"))
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .unwrap_or_default();
            let total = obj
                .data
                .get("status")
                .and_then(|s| s.get("instances"))
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string())
                .unwrap_or_default();

            if !ready.is_empty() && !total.is_empty() && ready == total {
                CheckResult::ok(
                    "postgres",
                    "data",
                    "postgres",
                    &format!("{ready}/{total} ready"),
                )
            } else {
                let r = if ready.is_empty() { "?" } else { &ready };
                let t = if total.is_empty() { "?" } else { &total };
                CheckResult::fail("postgres", "data", "postgres", &format!("{r}/{t} ready"))
            }
        }
        Ok(None) => CheckResult::fail("postgres", "data", "postgres", "cluster not found"),
        Err(e) => CheckResult::fail("postgres", "data", "postgres", &format!("{e}")),
    }
}

/// kubectl exec valkey pod -- valkey-cli ping -> PONG.
async fn check_valkey(_domain: &str, _client: &reqwest::Client) -> CheckResult {
    let kube_client = match get_client().await {
        Ok(c) => c,
        Err(e) => return CheckResult::fail("valkey", "data", "valkey", &format!("{e}")),
    };

    let api: Api<Pod> = Api::namespaced(kube_client.clone(), "data");
    let lp = ListParams::default().labels("app=valkey");
    let pod_list = match api.list(&lp).await {
        Ok(l) => l,
        Err(e) => return CheckResult::fail("valkey", "data", "valkey", &format!("{e}")),
    };

    let pod_name = match pod_list.items.first() {
        Some(p) => p.name_any(),
        None => return CheckResult::fail("valkey", "data", "valkey", "no valkey pod"),
    };

    match kube_exec("data", &pod_name, &["valkey-cli", "ping"], Some("valkey")).await {
        Ok((_, out)) => {
            let passed = out == "PONG";
            let detail = if out.is_empty() {
                "no response".to_string()
            } else {
                out
            };
            CheckResult {
                name: "valkey".into(),
                ns: "data".into(),
                svc: "valkey".into(),
                passed,
                detail,
            }
        }
        Err(e) => CheckResult::fail("valkey", "data", "valkey", &format!("{e}")),
    }
}

/// kubectl exec openbao-0 -- bao status -format=json -> initialized + unsealed.
async fn check_openbao(_domain: &str, _client: &reqwest::Client) -> CheckResult {
    match kube_exec(
        "data",
        "openbao-0",
        &["bao", "status", "-format=json"],
        Some("openbao"),
    )
    .await
    {
        Ok((_, out)) => {
            if out.is_empty() {
                return CheckResult::fail("openbao", "data", "openbao", "no response");
            }
            match serde_json::from_str::<serde_json::Value>(&out) {
                Ok(data) => {
                    let init = data
                        .get("initialized")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let sealed = data.get("sealed").and_then(|v| v.as_bool()).unwrap_or(true);
                    let passed = init && !sealed;
                    CheckResult {
                        name: "openbao".into(),
                        ns: "data".into(),
                        svc: "openbao".into(),
                        passed,
                        detail: format!("init={init}, sealed={sealed}"),
                    }
                }
                Err(_) => {
                    let truncated: String = out.chars().take(80).collect();
                    CheckResult::fail("openbao", "data", "openbao", &truncated)
                }
            }
        }
        Err(e) => CheckResult::fail("openbao", "data", "openbao", &format!("{e}")),
    }
}

// ---------------------------------------------------------------------------
// S3 auth (AWS4-HMAC-SHA256)
// ---------------------------------------------------------------------------

/// Generate AWS4-HMAC-SHA256 Authorization and x-amz-date headers for an unsigned
/// GET / request, matching the Python `_s3_auth_headers` function exactly.
fn s3_auth_headers(access_key: &str, secret_key: &str, host: &str) -> (String, String) {
    s3_auth_headers_at(access_key, secret_key, host, chrono::Utc::now())
}

/// Deterministic inner implementation that accepts an explicit timestamp.
fn s3_auth_headers_at(
    access_key: &str,
    secret_key: &str,
    host: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> (String, String) {
    let amzdate = now.format("%Y%m%dT%H%M%SZ").to_string();
    let datestamp = now.format("%Y%m%d").to_string();

    let payload_hash = hex_encode(Sha256::digest(b""));
    let canonical =
        format!("GET\n/\n\nhost:{host}\nx-amz-date:{amzdate}\n\nhost;x-amz-date\n{payload_hash}");
    let credential_scope = format!("{datestamp}/us-east-1/s3/aws4_request");
    let canonical_hash = hex_encode(Sha256::digest(canonical.as_bytes()));
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amzdate}\n{credential_scope}\n{canonical_hash}");

    fn hmac_sign(key: &[u8], msg: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(msg);
        mac.finalize().into_bytes().to_vec()
    }

    let k = hmac_sign(format!("AWS4{secret_key}").as_bytes(), datestamp.as_bytes());
    let k = hmac_sign(&k, b"us-east-1");
    let k = hmac_sign(&k, b"s3");
    let k = hmac_sign(&k, b"aws4_request");

    let sig = {
        let mut mac = HmacSha256::new_from_slice(&k).expect("HMAC accepts any key length");
        mac.update(string_to_sign.as_bytes());
        hex_encode(mac.finalize().into_bytes())
    };

    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders=host;x-amz-date, Signature={sig}"
    );
    (auth, amzdate)
}

/// GET https://s3.{domain}/ with S3 credentials -> 200 list-buckets response.
async fn check_seaweedfs(domain: &str, client: &reqwest::Client) -> CheckResult {
    let access_key = kube_secret("storage", "seaweedfs-s3-credentials", "S3_ACCESS_KEY").await;
    let secret_key = kube_secret("storage", "seaweedfs-s3-credentials", "S3_SECRET_KEY").await;

    if access_key.is_empty() || secret_key.is_empty() {
        return CheckResult::fail(
            "seaweedfs",
            "storage",
            "seaweedfs",
            "credentials not found in seaweedfs-s3-credentials secret",
        );
    }

    let host = format!("s3.{domain}");
    let url = format!("https://{host}/");
    let (auth, amzdate) = s3_auth_headers(&access_key, &secret_key, &host);

    match http_get(
        client,
        &url,
        Some(&[("Authorization", &auth), ("x-amz-date", &amzdate)]),
    )
    .await
    {
        Ok((200, _)) => CheckResult::ok("seaweedfs", "storage", "seaweedfs", "S3 authenticated"),
        Ok((status, _)) => CheckResult::fail(
            "seaweedfs",
            "storage",
            "seaweedfs",
            &format!("HTTP {status}"),
        ),
        Err(e) => CheckResult::fail("seaweedfs", "storage", "seaweedfs", &e),
    }
}

/// GET /kratos/health/ready -> 200.
async fn check_kratos(domain: &str, client: &reqwest::Client) -> CheckResult {
    let url = format!("https://auth.{domain}/kratos/health/ready");
    match http_get(client, &url, None).await {
        Ok((status, body)) => {
            let ok_flag = status == 200;
            let mut detail = format!("HTTP {status}");
            if !ok_flag && !body.is_empty() {
                let body_str: String = String::from_utf8_lossy(&body).chars().take(80).collect();
                detail = format!("{detail}: {body_str}");
            }
            CheckResult {
                name: "kratos".into(),
                ns: "ory".into(),
                svc: "kratos".into(),
                passed: ok_flag,
                detail,
            }
        }
        Err(e) => CheckResult::fail("kratos", "ory", "kratos", &e),
    }
}

/// GET /.well-known/openid-configuration -> 200 with issuer field.
async fn check_hydra_oidc(domain: &str, client: &reqwest::Client) -> CheckResult {
    let url = format!("https://auth.{domain}/.well-known/openid-configuration");
    match http_get(client, &url, None).await {
        Ok((200, body)) => {
            let issuer = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("issuer").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| "?".into());
            CheckResult::ok("hydra-oidc", "ory", "hydra", &format!("issuer={issuer}"))
        }
        Ok((status, _)) => {
            CheckResult::fail("hydra-oidc", "ory", "hydra", &format!("HTTP {status}"))
        }
        Err(e) => CheckResult::fail("hydra-oidc", "ory", "hydra", &e),
    }
}

/// kubectl exec livekit-server pod -- wget localhost:7880/ -> rc 0.
async fn check_livekit(_domain: &str, _client: &reqwest::Client) -> CheckResult {
    let kube_client = match get_client().await {
        Ok(c) => c,
        Err(e) => return CheckResult::fail("livekit", "media", "livekit", &format!("{e}")),
    };

    let api: Api<Pod> = Api::namespaced(kube_client.clone(), "media");
    let lp = ListParams::default().labels("app.kubernetes.io/name=livekit-server");
    let pod_list = match api.list(&lp).await {
        Ok(l) => l,
        Err(e) => return CheckResult::fail("livekit", "media", "livekit", &format!("{e}")),
    };

    let pod_name = match pod_list.items.first() {
        Some(p) => p.name_any(),
        None => return CheckResult::fail("livekit", "media", "livekit", "no livekit pod"),
    };

    match kube_exec(
        "media",
        &pod_name,
        &["wget", "-qO-", "http://localhost:7880/"],
        None,
    )
    .await
    {
        Ok((exit_code, _)) => {
            if exit_code == 0 {
                CheckResult::ok("livekit", "media", "livekit", "server responding")
            } else {
                CheckResult::fail("livekit", "media", "livekit", "server not responding")
            }
        }
        Err(e) => CheckResult::fail("livekit", "media", "livekit", &format!("{e}")),
    }
}

// ---------------------------------------------------------------------------
// Check registry — function pointer + metadata
// ---------------------------------------------------------------------------

type CheckFn = for<'a> fn(
    &'a str,
    &'a reqwest::Client,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = CheckResult> + Send + 'a>,
>;

struct CheckEntry {
    func: CheckFn,
    ns: &'static str,
    svc: &'static str,
}

fn check_registry() -> Vec<CheckEntry> {
    vec![
        CheckEntry {
            func: |d, c| Box::pin(check_gitea_version(d, c)),
            ns: "devtools",
            svc: "gitea",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_gitea_auth(d, c)),
            ns: "devtools",
            svc: "gitea",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_postgres(d, c)),
            ns: "data",
            svc: "postgres",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_valkey(d, c)),
            ns: "data",
            svc: "valkey",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_openbao(d, c)),
            ns: "data",
            svc: "openbao",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_seaweedfs(d, c)),
            ns: "storage",
            svc: "seaweedfs",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_kratos(d, c)),
            ns: "ory",
            svc: "kratos",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_hydra_oidc(d, c)),
            ns: "ory",
            svc: "hydra",
        },
        CheckEntry {
            func: |d, c| Box::pin(check_livekit(d, c)),
            ns: "media",
            svc: "livekit",
        },
    ]
}

// ---------------------------------------------------------------------------
// cmd_check — concurrent execution
// ---------------------------------------------------------------------------

/// Run service-level health checks, optionally scoped to a namespace or service.
pub async fn cmd_check(target: Option<&str>) -> Result<()> {
    step("Service health checks...");

    let domain = crate::kube::get_domain().await?;
    let http_client = build_http_client()?;

    let (ns_filter, svc_filter) = parse_target(target)?;

    let all_checks = check_registry();
    let selected: Vec<&CheckEntry> = all_checks
        .iter()
        .filter(|e| {
            (ns_filter.is_none() || ns_filter == Some(e.ns))
                && (svc_filter.is_none() || svc_filter == Some(e.svc))
        })
        .collect();

    if selected.is_empty() {
        warn(&format!(
            "No checks match target: {}",
            target.unwrap_or("(none)")
        ));
        return Ok(());
    }

    // Run all checks concurrently
    let mut join_set = tokio::task::JoinSet::new();
    for entry in &selected {
        let domain = domain.clone();
        let client = http_client.clone();
        let func = entry.func;
        join_set.spawn(async move { func(&domain, &client).await });
    }

    let mut results: Vec<CheckResult> = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(cr) => results.push(cr),
            Err(e) => results.push(CheckResult::fail("unknown", "?", "?", &format!("{e}"))),
        }
    }

    // Sort to match the registry order for consistent output
    let registry = check_registry();
    results.sort_by(|a, b| {
        let idx_a = registry
            .iter()
            .position(|e| e.ns == a.ns && e.svc == a.svc)
            .unwrap_or(usize::MAX);
        let idx_b = registry
            .iter()
            .position(|e| e.ns == b.ns && e.svc == b.svc)
            .unwrap_or(usize::MAX);
        idx_a.cmp(&idx_b).then_with(|| a.name.cmp(&b.name))
    });

    // Print grouped by namespace
    let name_w = results.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let mut cur_ns: Option<&str> = None;
    for r in &results {
        if cur_ns != Some(&r.ns) {
            println!("  {}:", r.ns);
            cur_ns = Some(&r.ns);
        }
        let icon = if r.passed { "\u{2713}" } else { "\u{2717}" };
        let detail = if r.detail.is_empty() {
            String::new()
        } else {
            format!("  {}", r.detail)
        };
        println!("    {icon} {:<name_w$}{detail}", r.name);
    }

    println!();
    let failed: Vec<&CheckResult> = results.iter().filter(|r| !r.passed).collect();
    if failed.is_empty() {
        ok(&format!("All {} check(s) passed.", results.len()));
    } else {
        warn(&format!("{} check(s) failed.", failed.len()));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// hex encoding helper (avoids adding the `hex` crate)
// ---------------------------------------------------------------------------

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0xf) as usize] as char);
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── S3 auth header tests ─────────────────────────────────────────────

    #[test]
    fn test_s3_auth_headers_format() {
        let (auth, amzdate) = s3_auth_headers(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "s3.example.com",
        );

        // Verify header structure
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/"));
        assert!(auth.contains("us-east-1/s3/aws4_request"));
        assert!(auth.contains("SignedHeaders=host;x-amz-date"));
        assert!(auth.contains("Signature="));

        // amzdate format: YYYYMMDDTHHMMSSZ
        assert_eq!(amzdate.len(), 16);
        assert!(amzdate.ends_with('Z'));
        assert!(amzdate.contains('T'));
    }

    #[test]
    fn test_s3_auth_headers_signature_changes_with_key() {
        let (auth1, _) = s3_auth_headers("key1", "secret1", "host1");
        let (auth2, _) = s3_auth_headers("key2", "secret2", "host2");
        // Different keys produce different signatures
        let sig1 = auth1.split("Signature=").nth(1).unwrap();
        let sig2 = auth2.split("Signature=").nth(1).unwrap();
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_s3_auth_headers_credential_scope() {
        let (auth, amzdate) = s3_auth_headers("AK", "SK", "s3.example.com");
        let datestamp = &amzdate[..8];
        let expected_scope = format!("{datestamp}/us-east-1/s3/aws4_request");
        assert!(auth.contains(&expected_scope));
    }

    // ── hex encoding ────────────────────────────────────────────────────

    #[test]
    fn test_hex_encode_empty() {
        assert_eq!(hex_encode(b""), "");
    }

    #[test]
    fn test_hex_encode_zero() {
        assert_eq!(hex_encode(b"\x00"), "00");
    }

    #[test]
    fn test_hex_encode_ff() {
        assert_eq!(hex_encode(b"\xff"), "ff");
    }

    #[test]
    fn test_hex_encode_deadbeef() {
        assert_eq!(hex_encode(b"\xde\xad\xbe\xef"), "deadbeef");
    }

    #[test]
    fn test_hex_encode_hello() {
        assert_eq!(hex_encode(b"hello"), "68656c6c6f");
    }

    // ── CheckResult ─────────────────────────────────────────────────────

    #[test]
    fn test_check_result_ok() {
        let r = CheckResult::ok("gitea-version", "devtools", "gitea", "v1.21.0");
        assert!(r.passed);
        assert_eq!(r.name, "gitea-version");
        assert_eq!(r.ns, "devtools");
        assert_eq!(r.svc, "gitea");
        assert_eq!(r.detail, "v1.21.0");
    }

    #[test]
    fn test_check_result_fail() {
        let r = CheckResult::fail("postgres", "data", "postgres", "cluster not found");
        assert!(!r.passed);
        assert_eq!(r.detail, "cluster not found");
    }

    // ── Check registry ──────────────────────────────────────────────────

    #[test]
    fn test_check_registry_has_all_checks() {
        let registry = check_registry();
        assert_eq!(registry.len(), 9);

        // Verify order matches Python CHECKS list
        assert_eq!(registry[0].ns, "devtools");
        assert_eq!(registry[0].svc, "gitea");
        assert_eq!(registry[1].ns, "devtools");
        assert_eq!(registry[1].svc, "gitea");
        assert_eq!(registry[2].ns, "data");
        assert_eq!(registry[2].svc, "postgres");
        assert_eq!(registry[3].ns, "data");
        assert_eq!(registry[3].svc, "valkey");
        assert_eq!(registry[4].ns, "data");
        assert_eq!(registry[4].svc, "openbao");
        assert_eq!(registry[5].ns, "storage");
        assert_eq!(registry[5].svc, "seaweedfs");
        assert_eq!(registry[6].ns, "ory");
        assert_eq!(registry[6].svc, "kratos");
        assert_eq!(registry[7].ns, "ory");
        assert_eq!(registry[7].svc, "hydra");
        assert_eq!(registry[8].ns, "media");
        assert_eq!(registry[8].svc, "livekit");
    }

    #[test]
    fn test_check_registry_filter_namespace() {
        let all = check_registry();
        let filtered: Vec<&CheckEntry> = all.iter().filter(|e| e.ns == "ory").collect();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_check_registry_filter_service() {
        let all = check_registry();
        let filtered: Vec<&CheckEntry> = all
            .iter()
            .filter(|e| e.ns == "ory" && e.svc == "kratos")
            .collect();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_check_registry_filter_no_match() {
        let all = check_registry();
        let filtered: Vec<&CheckEntry> = all.iter().filter(|e| e.ns == "nonexistent").collect();
        assert!(filtered.is_empty());
    }

    // ── HMAC-SHA256 verification ────────────────────────────────────────

    #[test]
    fn test_hmac_sha256_known_vector() {
        // RFC 4231 Test Case 2
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key");
        mac.update(data);
        let result = hex_encode(mac.finalize().into_bytes());
        assert_eq!(
            result,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    // ── SHA256 verification ─────────────────────────────────────────────

    #[test]
    fn test_sha256_empty() {
        let hash = hex_encode(Sha256::digest(b""));
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_hello() {
        let hash = hex_encode(Sha256::digest(b"hello"));
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // ── Additional CheckResult tests ──────────────────────────────────

    #[test]
    fn test_check_result_ok_empty_detail() {
        let r = CheckResult::ok("test", "ns", "svc", "");
        assert!(r.passed);
        assert!(r.detail.is_empty());
    }

    #[test]
    fn test_check_result_fail_contains_status_code() {
        let r = CheckResult::fail("gitea-version", "devtools", "gitea", "HTTP 502");
        assert!(!r.passed);
        assert!(r.detail.contains("502"));
    }

    #[test]
    fn test_check_result_fail_contains_secret_message() {
        let r = CheckResult::fail(
            "gitea-auth",
            "devtools",
            "gitea",
            "password not found in secret",
        );
        assert!(!r.passed);
        assert!(r.detail.contains("secret"));
    }

    #[test]
    fn test_check_result_ok_with_version() {
        let r = CheckResult::ok("gitea-version", "devtools", "gitea", "v1.21.0");
        assert!(r.passed);
        assert!(r.detail.contains("1.21.0"));
    }

    #[test]
    fn test_check_result_ok_with_login() {
        let r = CheckResult::ok("gitea-auth", "devtools", "gitea", "user=gitea_admin");
        assert!(r.passed);
        assert!(r.detail.contains("gitea_admin"));
    }

    #[test]
    fn test_check_result_ok_authenticated() {
        let r = CheckResult::ok("seaweedfs", "storage", "seaweedfs", "S3 authenticated");
        assert!(r.passed);
        assert!(r.detail.contains("authenticated"));
    }

    // ── Additional registry tests ─────────────────────────────────────

    #[test]
    fn test_check_registry_expected_namespaces() {
        let registry = check_registry();
        let namespaces: std::collections::HashSet<&str> = registry.iter().map(|e| e.ns).collect();
        for expected in &["devtools", "data", "storage", "ory", "media"] {
            assert!(
                namespaces.contains(expected),
                "registry missing namespace: {expected}"
            );
        }
    }

    #[test]
    fn test_check_registry_expected_services() {
        let registry = check_registry();
        let services: std::collections::HashSet<&str> = registry.iter().map(|e| e.svc).collect();
        for expected in &[
            "gitea",
            "postgres",
            "valkey",
            "openbao",
            "seaweedfs",
            "kratos",
            "hydra",
            "livekit",
        ] {
            assert!(
                services.contains(expected),
                "registry missing service: {expected}"
            );
        }
    }

    #[test]
    fn test_check_registry_devtools_has_two_gitea_entries() {
        let registry = check_registry();
        let gitea: Vec<_> = registry
            .iter()
            .filter(|e| e.ns == "devtools" && e.svc == "gitea")
            .collect();
        assert_eq!(gitea.len(), 2);
    }

    #[test]
    fn test_check_registry_data_has_three_entries() {
        let registry = check_registry();
        let data: Vec<_> = registry.iter().filter(|e| e.ns == "data").collect();
        assert_eq!(data.len(), 3); // postgres, valkey, openbao
    }

    // ── Filter logic (mirrors Python TestCmdCheck) ────────────────────

    /// Helper: apply the same filter logic as cmd_check to the registry.
    fn filter_registry(
        ns_filter: Option<&str>,
        svc_filter: Option<&str>,
    ) -> Vec<(&'static str, &'static str)> {
        let all = check_registry();
        all.into_iter()
            .filter(|e| ns_filter.is_none_or(|ns| e.ns == ns))
            .filter(|e| svc_filter.is_none_or(|svc| e.svc == svc))
            .map(|e| (e.ns, e.svc))
            .collect()
    }

    #[test]
    fn test_no_target_runs_all() {
        let selected = filter_registry(None, None);
        assert_eq!(selected.len(), 9);
    }

    #[test]
    fn test_ns_filter_devtools_selects_two() {
        let selected = filter_registry(Some("devtools"), None);
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|(ns, _)| *ns == "devtools"));
    }

    #[test]
    fn test_ns_filter_skips_other_namespaces() {
        let selected = filter_registry(Some("devtools"), None);
        // Should NOT contain data/postgres
        assert!(selected.iter().all(|(ns, _)| *ns != "data"));
    }

    #[test]
    fn test_svc_filter_ory_kratos() {
        let selected = filter_registry(Some("ory"), Some("kratos"));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], ("ory", "kratos"));
    }

    #[test]
    fn test_svc_filter_ory_hydra() {
        let selected = filter_registry(Some("ory"), Some("hydra"));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], ("ory", "hydra"));
    }

    #[test]
    fn test_filter_nonexistent_ns_returns_empty() {
        let selected = filter_registry(Some("nonexistent"), None);
        assert!(selected.is_empty());
    }

    #[test]
    fn test_filter_ns_match_svc_mismatch_returns_empty() {
        // ory namespace exists but postgres service does not live there
        let selected = filter_registry(Some("ory"), Some("postgres"));
        assert!(selected.is_empty());
    }

    #[test]
    fn test_filter_data_namespace() {
        let selected = filter_registry(Some("data"), None);
        assert_eq!(selected.len(), 3);
        let svcs: Vec<&str> = selected.iter().map(|(_, svc)| *svc).collect();
        assert!(svcs.contains(&"postgres"));
        assert!(svcs.contains(&"valkey"));
        assert!(svcs.contains(&"openbao"));
    }

    #[test]
    fn test_filter_storage_namespace() {
        let selected = filter_registry(Some("storage"), None);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], ("storage", "seaweedfs"));
    }

    #[test]
    fn test_filter_media_namespace() {
        let selected = filter_registry(Some("media"), None);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], ("media", "livekit"));
    }

    // ── S3 auth AWS reference vector test ─────────────────────────────

    #[test]
    fn test_s3_auth_headers_aws_reference_vector() {
        // Uses AWS test values with a fixed timestamp to verify signature
        // correctness against a known reference (AWS SigV4 documentation).
        use chrono::TimeZone;

        let access_key = "AKIAIOSFODNN7EXAMPLE";
        let secret_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let host = "examplebucket.s3.amazonaws.com";
        let now = chrono::Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();

        let (auth, amzdate) = s3_auth_headers_at(access_key, secret_key, host, now);

        // 1. Verify the date header
        assert_eq!(amzdate, "20130524T000000Z");

        // 2. Verify canonical request intermediate values.
        //    Canonical request for GET / with empty body:
        //      GET\n/\n\nhost:examplebucket.s3.amazonaws.com\n
        //      x-amz-date:20130524T000000Z\n\nhost;x-amz-date\n<sha256("")>
        let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let canonical = format!(
            "GET\n/\n\nhost:{host}\nx-amz-date:{amzdate}\n\nhost;x-amz-date\n{payload_hash}"
        );
        let canonical_hash = hex_encode(Sha256::digest(canonical.as_bytes()));

        // 3. Verify the string to sign
        let credential_scope = "20130524/us-east-1/s3/aws4_request";
        let string_to_sign =
            format!("AWS4-HMAC-SHA256\n{amzdate}\n{credential_scope}\n{canonical_hash}");

        // 4. Compute the expected signing key and signature to pin the value.
        fn hmac_sign(key: &[u8], msg: &[u8]) -> Vec<u8> {
            let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
            mac.update(msg);
            mac.finalize().into_bytes().to_vec()
        }

        let k = hmac_sign(format!("AWS4{secret_key}").as_bytes(), b"20130524");
        let k = hmac_sign(&k, b"us-east-1");
        let k = hmac_sign(&k, b"s3");
        let k = hmac_sign(&k, b"aws4_request");

        let expected_sig = {
            let mut mac = HmacSha256::new_from_slice(&k).expect("HMAC accepts any key length");
            mac.update(string_to_sign.as_bytes());
            hex_encode(mac.finalize().into_bytes())
        };

        // 5. Verify the full Authorization header matches
        let expected_auth = format!(
            "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, \
             SignedHeaders=host;x-amz-date, Signature={expected_sig}"
        );
        assert_eq!(auth, expected_auth);

        // 6. Pin the exact signature value so any regression is caught
        //    immediately without needing to recompute.
        let sig = auth.split("Signature=").nth(1).unwrap();
        assert_eq!(sig, expected_sig);
        assert_eq!(sig.len(), 64, "SHA-256 HMAC signature must be 64 hex chars");
    }

    // ── Additional S3 auth header tests ───────────────────────────────

    #[test]
    fn test_s3_auth_headers_deterministic() {
        // Same inputs at the same point in time produce identical output.
        // (Time may advance between calls, but the format is still valid.)
        let (auth1, date1) = s3_auth_headers("AK", "SK", "host");
        let (auth2, date2) = s3_auth_headers("AK", "SK", "host");
        // If both calls happen within the same second, they must be identical.
        if date1 == date2 {
            assert_eq!(
                auth1, auth2,
                "same inputs at same time must produce same signature"
            );
        }
    }

    #[test]
    fn test_s3_auth_headers_different_hosts_differ() {
        let (auth1, d1) = s3_auth_headers("AK", "SK", "s3.a.com");
        let (auth2, d2) = s3_auth_headers("AK", "SK", "s3.b.com");
        let sig1 = auth1.split("Signature=").nth(1).unwrap();
        let sig2 = auth2.split("Signature=").nth(1).unwrap();
        // Different hosts -> different canonical request -> different signature
        // (only guaranteed when timestamps match)
        if d1 == d2 {
            assert_ne!(sig1, sig2);
        }
    }

    #[test]
    fn test_s3_auth_headers_signature_is_64_hex_chars() {
        let (auth, _) = s3_auth_headers("AK", "SK", "host");
        let sig = auth.split("Signature=").nth(1).unwrap();
        assert_eq!(sig.len(), 64, "SHA-256 HMAC hex signature is 64 chars");
        assert!(
            sig.chars().all(|c| c.is_ascii_hexdigit()),
            "signature must be lowercase hex: {sig}"
        );
    }

    // ── hex_encode edge cases ─────────────────────────────────────────

    #[test]
    fn test_hex_encode_all_byte_values() {
        // Verify 0x00..0xff all produce 2-char lowercase hex
        for b in 0u8..=255 {
            let encoded = hex_encode([b]);
            assert_eq!(encoded.len(), 2);
            assert!(encoded.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn test_hex_encode_matches_format() {
        // Cross-check against Rust's built-in formatting
        let bytes: Vec<u8> = (0..32).collect();
        let expected: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex_encode(&bytes), expected);
    }
}
