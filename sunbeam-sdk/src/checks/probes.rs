//! Individual service health check probe functions.

use base64::Engine;
use hmac::{Hmac, Mac};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use kube::ResourceExt;
use sha2::{Digest, Sha256};

use super::{CheckResult, http_get, kube_secret};
use crate::kube::{get_client, kube_exec};

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

/// GET /api/v1/version -> JSON with version field.
pub(super) async fn check_gitea_version(domain: &str, client: &reqwest::Client) -> CheckResult {
    let url = format!("https://src.{domain}/api/v1/version");
    match http_get(client, &url, None).await {
        Ok((200, body)) => {
            let ver = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| "?".into());
            CheckResult::ok("gitea-version", "devtools", "gitea", &format!("v{ver}"))
        }
        Ok((status, _)) => {
            CheckResult::fail("gitea-version", "devtools", "gitea", &format!("HTTP {status}"))
        }
        Err(e) => CheckResult::fail("gitea-version", "devtools", "gitea", &e),
    }
}

/// GET /api/v1/user with admin credentials -> 200 and login field.
pub(super) async fn check_gitea_auth(domain: &str, client: &reqwest::Client) -> CheckResult {
    let username = {
        let u = kube_secret("devtools", "gitea-admin-credentials", "username").await;
        if u.is_empty() {
            "gitea_admin".to_string()
        } else {
            u
        }
    };
    let password =
        kube_secret("devtools", "gitea-admin-credentials", "password").await;
    if password.is_empty() {
        return CheckResult::fail(
            "gitea-auth",
            "devtools",
            "gitea",
            "password not found in secret",
        );
    }

    let creds =
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
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
pub(super) async fn check_postgres(_domain: &str, _client: &reqwest::Client) -> CheckResult {
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

    let api: Api<kube::api::DynamicObject> =
        Api::namespaced_with(kube_client.clone(), "data", &ar);

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
pub(super) async fn check_valkey(_domain: &str, _client: &reqwest::Client) -> CheckResult {
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
pub(super) async fn check_openbao(_domain: &str, _client: &reqwest::Client) -> CheckResult {
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
                    let sealed = data
                        .get("sealed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
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
pub(crate) fn s3_auth_headers(access_key: &str, secret_key: &str, host: &str) -> (String, String) {
    s3_auth_headers_at(access_key, secret_key, host, chrono::Utc::now())
}

/// Deterministic inner implementation that accepts an explicit timestamp.
pub(crate) fn s3_auth_headers_at(
    access_key: &str,
    secret_key: &str,
    host: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> (String, String) {
    let amzdate = now.format("%Y%m%dT%H%M%SZ").to_string();
    let datestamp = now.format("%Y%m%d").to_string();

    let payload_hash = hex_encode(&Sha256::digest(b""));
    let canonical = format!(
        "GET\n/\n\nhost:{host}\nx-amz-date:{amzdate}\n\nhost;x-amz-date\n{payload_hash}"
    );
    let credential_scope = format!("{datestamp}/us-east-1/s3/aws4_request");
    let canonical_hash = hex_encode(&Sha256::digest(canonical.as_bytes()));
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amzdate}\n{credential_scope}\n{canonical_hash}");

    fn hmac_sign(key: &[u8], msg: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(msg);
        mac.finalize().into_bytes().to_vec()
    }

    let k = hmac_sign(
        format!("AWS4{secret_key}").as_bytes(),
        datestamp.as_bytes(),
    );
    let k = hmac_sign(&k, b"us-east-1");
    let k = hmac_sign(&k, b"s3");
    let k = hmac_sign(&k, b"aws4_request");

    let sig = {
        let mut mac = HmacSha256::new_from_slice(&k).expect("HMAC accepts any key length");
        mac.update(string_to_sign.as_bytes());
        hex_encode(&mac.finalize().into_bytes())
    };

    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders=host;x-amz-date, Signature={sig}"
    );
    (auth, amzdate)
}

/// GET https://s3.{domain}/ with S3 credentials -> 200 list-buckets response.
pub(super) async fn check_seaweedfs(domain: &str, client: &reqwest::Client) -> CheckResult {
    let access_key =
        kube_secret("storage", "seaweedfs-s3-credentials", "S3_ACCESS_KEY").await;
    let secret_key =
        kube_secret("storage", "seaweedfs-s3-credentials", "S3_SECRET_KEY").await;

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
        Ok((200, _)) => {
            CheckResult::ok("seaweedfs", "storage", "seaweedfs", "S3 authenticated")
        }
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
pub(super) async fn check_kratos(domain: &str, client: &reqwest::Client) -> CheckResult {
    let url = format!("https://auth.{domain}/kratos/health/ready");
    match http_get(client, &url, None).await {
        Ok((status, body)) => {
            let ok_flag = status == 200;
            let mut detail = format!("HTTP {status}");
            if !ok_flag && !body.is_empty() {
                let body_str: String =
                    String::from_utf8_lossy(&body).chars().take(80).collect();
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
pub(super) async fn check_hydra_oidc(domain: &str, client: &reqwest::Client) -> CheckResult {
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
pub(super) async fn check_livekit(_domain: &str, _client: &reqwest::Client) -> CheckResult {
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
// hex encoding helper (avoids adding the `hex` crate)
// ---------------------------------------------------------------------------

pub(crate) fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0xf) as usize] as char);
    }
    s
}
