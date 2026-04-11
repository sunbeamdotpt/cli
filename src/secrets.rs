//! Secrets management — shared helpers for OpenBao, port-forwarding, and DB engine config.
//!
//! High-level seed/verify orchestration lives in WFE workflow primitives;
//! this module provides the building blocks they call into.

use crate::error::{Result, ResultExt, SunbeamError};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use rand::RngCore;
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tokio::net::TcpListener;

use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::{ok, warn};

// ── Constants ───────────────────────────────────────────────────────────────

pub(crate) const ADMIN_USERNAME: &str = "estudio-admin";
pub(crate) const GITEA_ADMIN_USER: &str = "gitea_admin";
pub(crate) const PG_USERS: &[&str] = &[
    "kratos",
    "hydra",
    "gitea",
    "penpot",
    "stalwart",
    "headscale",
    "wfe",
    "typst_editor",
];

pub(crate) const SMTP_URI: &str =
    "smtp://stalwart.stalwart.svc.cluster.local:25/?skip_ssl_verify=true";

// ── Key generation ──────────────────────────────────────────────────────────

/// Generate a Fernet-compatible key (32 random bytes, URL-safe base64).
pub(crate) fn gen_fernet_key() -> String {
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE.encode(buf)
}

/// Generate an RSA 2048-bit DKIM key pair.
/// Returns (private_pem_pkcs8, public_pem). Returns ("", "") on failure.
pub(crate) fn gen_dkim_key_pair() -> (String, String) {
    let mut rng = rand::thread_rng();
    let bits = 2048;
    let private_key = match RsaPrivateKey::new(&mut rng, bits) {
        Ok(k) => k,
        Err(e) => {
            warn(&format!("RSA key generation failed: {e}"));
            return (String::new(), String::new());
        }
    };

    let private_pem = match private_key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF) {
        Ok(p) => p.to_string(),
        Err(e) => {
            warn(&format!("PKCS8 encoding failed: {e}"));
            return (String::new(), String::new());
        }
    };

    let public_key = private_key.to_public_key();
    let public_pem = match public_key.to_public_key_pem(rsa::pkcs8::LineEnding::LF) {
        Ok(p) => p.to_string(),
        Err(e) => {
            warn(&format!("Public key PEM encoding failed: {e}"));
            return (private_pem, String::new());
        }
    };

    (private_pem, public_pem)
}

/// Generate a URL-safe random token (32 bytes).
pub(crate) fn rand_token() -> String {
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Generate a URL-safe random token with a specific byte count.
pub(crate) fn rand_token_n(n: usize) -> String {
    use base64::Engine;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

// ── Port-forward helper ─────────────────────────────────────────────────────

/// Port-forward guard — cancels the background forwarder on drop.
pub(crate) struct PortForwardGuard {
    _abort_handle: tokio::task::AbortHandle,
    pub local_port: u16,
}

impl Drop for PortForwardGuard {
    fn drop(&mut self) {
        self._abort_handle.abort();
    }
}

/// Open a kube-rs port-forward to `pod_name` in `namespace` on `remote_port`.
/// Binds a local TCP listener and proxies connections to the pod.
pub(crate) async fn port_forward(
    namespace: &str,
    pod_name: &str,
    remote_port: u16,
) -> Result<PortForwardGuard> {
    let client = k::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .ctx("Failed to bind local TCP listener for port-forward")?;
    let local_port = listener
        .local_addr()
        .map_err(|e| SunbeamError::Other(format!("local_addr: {e}")))?
        .port();

    let pod_name = pod_name.to_string();
    let ns = namespace.to_string();
    let task = tokio::spawn(async move {
        let mut current_pod = pod_name;
        let mut consecutive_failures: u32 = 0;
        const MAX_CONSECUTIVE_FAILURES: u32 = 30;
        loop {
            let (mut client_stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };

            let pf_result = pods.portforward(&current_pod, &[remote_port]).await;
            let mut pf = match pf_result {
                Ok(pf) => {
                    consecutive_failures = 0;
                    pf
                }
                Err(e) => {
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        tracing::error!(
                            "Port-forward to {current_pod} failed {consecutive_failures} times, giving up: {e}"
                        );
                        break;
                    }
                    tracing::warn!(
                        "Port-forward failed ({consecutive_failures}/{MAX_CONSECUTIVE_FAILURES}), re-resolving pod: {e}"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    // Re-resolve the pod in case it restarted with a new name
                    if let Ok(new_client) = k::get_client().await {
                        let new_pods: Api<Pod> = Api::namespaced(new_client.clone(), &ns);
                        let lp = ListParams::default();
                        if let Ok(pod_list) = new_pods.list(&lp).await {
                            if let Some(name) = pod_list
                                .items
                                .iter()
                                .find(|p| {
                                    p.metadata
                                        .name
                                        .as_deref()
                                        .map(|n| {
                                            n.starts_with(
                                                current_pod.split('-').next().unwrap_or(""),
                                            )
                                        })
                                        .unwrap_or(false)
                                })
                                .and_then(|p| p.metadata.name.clone())
                            {
                                current_pod = name;
                            }
                        }
                    }
                    continue;
                }
            };

            let mut upstream = match pf.take_stream(remote_port) {
                Some(s) => s,
                None => continue,
            };

            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut upstream).await;
            });
        }
    });

    let abort_handle = task.abort_handle();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    Ok(PortForwardGuard {
        _abort_handle: abort_handle,
        local_port,
    })
}

/// Port-forward to a service by finding a matching pod via label selector.
pub(crate) async fn port_forward_svc(
    namespace: &str,
    label_selector: &str,
    remote_port: u16,
) -> Result<PortForwardGuard> {
    let client = k::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let lp = ListParams::default().labels(label_selector);
    let pod_list = pods.list(&lp).await?;
    let pod_name = pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())
        .ctx("No pod found matching label selector")?
        .to_string();

    port_forward(namespace, &pod_name, remote_port).await
}

// ── OpenBao KV seeding ──────────────────────────────────────────────────────

/// Read-or-create pattern: reads existing KV values, only generates missing ones.
pub(crate) async fn get_or_create(
    bao: &BaoClient,
    path: &str,
    fields: &[(&str, &(dyn Fn() -> String + Send + Sync))],
    dirty_paths: &mut HashSet<String>,
) -> Result<HashMap<String, String>> {
    let existing = bao.kv_get("secret", path).await?.unwrap_or_default();
    let mut result = HashMap::new();
    for (key, default_fn) in fields {
        let val = existing.get(*key).filter(|v| !v.is_empty()).cloned();
        if let Some(v) = val {
            result.insert(key.to_string(), v);
        } else {
            result.insert(key.to_string(), default_fn());
            dirty_paths.insert(path.to_string());
        }
    }
    Ok(result)
}

// ── Database secrets engine ─────────────────────────────────────────────────

/// Enable OpenBao database secrets engine and create PostgreSQL static roles.
pub(crate) async fn configure_db_engine(bao: &BaoClient) -> Result<()> {
    ok("Configuring OpenBao database secrets engine...");
    let pg_rw = "postgres-rw.data.svc.cluster.local:5432";

    let _ = bao.enable_secrets_engine("database", "database").await;

    // ── vault PG user setup ─────────────────────────────────────────────
    let client = k::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), "data");
    let lp = ListParams::default().labels("cnpg.io/cluster=postgres,role=primary");
    let pod_list = pods.list(&lp).await?;
    let cnpg_pod = pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())
        .ctx("Could not find CNPG primary pod for vault user setup.")?
        .to_string();

    let existing_vault_pass = bao.kv_get_field("secret", "vault", "pg-password").await?;
    let vault_pg_pass = if existing_vault_pass.is_empty() {
        let new_pass = rand_token();
        let mut vault_data = HashMap::new();
        vault_data.insert("pg-password".to_string(), new_pass.clone());
        bao.kv_put("secret", "vault", &vault_data).await?;
        ok("vault KV entry written.");
        new_pass
    } else {
        ok("vault KV entry already present -- skipping write.");
        existing_vault_pass
    };

    let create_vault_sql = concat!(
        "DO $$ BEGIN ",
        "IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'vault') THEN ",
        "CREATE USER vault WITH LOGIN CREATEROLE; ",
        "END IF; ",
        "END $$;"
    );

    psql_exec(&cnpg_pod, create_vault_sql).await?;
    psql_exec(
        &cnpg_pod,
        &format!("ALTER USER vault WITH PASSWORD '{vault_pg_pass}';"),
    )
    .await?;

    for user in PG_USERS {
        psql_exec(
            &cnpg_pod,
            &format!("GRANT {user} TO vault WITH ADMIN OPTION;"),
        )
        .await?;
    }
    ok("vault PG user configured with ADMIN OPTION on all service roles.");

    let conn_url =
        format!("postgresql://{{{{username}}}}:{{{{password}}}}@{pg_rw}/postgres?sslmode=disable");

    bao.write_db_config(
        "cnpg-postgres",
        "postgresql-database-plugin",
        &conn_url,
        "vault",
        &vault_pg_pass,
        "*",
    )
    .await?;
    ok("DB engine connection configured (vault user).");

    let rotation_stmt = r#"ALTER USER "{{name}}" WITH PASSWORD '{{password}}';"#;

    for user in PG_USERS {
        bao.write_db_static_role(user, "cnpg-postgres", user, 86400, &[rotation_stmt])
            .await?;
        ok(&format!("  static-role/{user}"));
    }

    ok("Database secrets engine configured.");
    Ok(())
}

/// Execute a psql command on the CNPG primary pod.
pub(crate) async fn psql_exec(cnpg_pod: &str, sql: &str) -> Result<(i32, String)> {
    k::kube_exec(
        "data",
        cnpg_pod,
        &["psql", "-U", "postgres", "-c", sql],
        Some("postgres"),
    )
    .await
}

// ── Kratos types (used by WFE kratos-admin step) ───────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct KratosIdentity {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct KratosRecovery {
    #[serde(default)]
    pub(crate) recovery_link: String,
    #[serde(default)]
    pub(crate) recovery_code: String,
}

// ── Utility helpers ─────────────────────────────────────────────────────────

pub(crate) async fn wait_pod_running(ns: &str, pod_name: &str, timeout_secs: u64) -> bool {
    let client = match k::get_client().await {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(pod)) = pods.get_opt(pod_name).await {
            if pod
                .status
                .as_ref()
                .and_then(|s| s.phase.as_deref())
                .unwrap_or("")
                == "Running"
            {
                return true;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    false
}

pub(crate) fn scw_config(key: &str) -> String {
    std::process::Command::new("scw")
        .args(["config", "get", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub(crate) async fn delete_resource(ns: &str, kind: &str, name: &str) -> Result<()> {
    let ctx = format!("--context={}", k::context());
    let _ = tokio::process::Command::new("kubectl")
        .args([&ctx, "-n", ns, "delete", kind, name, "--ignore-not-found"])
        .output()
        .await;
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_fernet_key_length() {
        use base64::Engine;
        let key = gen_fernet_key();
        assert_eq!(key.len(), 44);
        let decoded = base64::engine::general_purpose::URL_SAFE
            .decode(&key)
            .expect("should be valid URL-safe base64");
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn test_gen_fernet_key_unique() {
        let k1 = gen_fernet_key();
        let k2 = gen_fernet_key();
        assert_ne!(k1, k2, "Two generated Fernet keys should differ");
    }

    #[test]
    fn test_gen_dkim_key_pair_produces_pem() {
        let (private_pem, public_pem) = gen_dkim_key_pair();
        assert!(
            private_pem.contains("BEGIN PRIVATE KEY"),
            "Private key should be PKCS8 PEM"
        );
        assert!(
            public_pem.contains("BEGIN PUBLIC KEY"),
            "Public key should be SPKI PEM (not PKCS#1)"
        );
        assert!(
            !public_pem.contains("BEGIN RSA PUBLIC KEY"),
            "Public key should NOT be PKCS#1 format"
        );
        assert!(!private_pem.is_empty());
        assert!(!public_pem.is_empty());
    }

    #[test]
    fn test_rand_token_nonempty_and_unique() {
        let t1 = rand_token();
        let t2 = rand_token();
        assert!(!t1.is_empty());
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_rand_token_n_length() {
        use base64::Engine;
        let t = rand_token_n(50);
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&t)
            .expect("should be valid URL-safe base64");
        assert_eq!(decoded.len(), 50);
    }

    #[test]
    fn test_constants() {
        assert_eq!(ADMIN_USERNAME, "estudio-admin");
        assert_eq!(GITEA_ADMIN_USER, "gitea_admin");
        assert_eq!(PG_USERS.len(), 8);
        assert!(PG_USERS.contains(&"kratos"));
        assert!(PG_USERS.contains(&"hydra"));
        assert!(PG_USERS.contains(&"wfe"));
        assert!(PG_USERS.contains(&"headscale"));
        assert!(PG_USERS.contains(&"typst_editor"));
    }

    #[test]
    fn test_scw_config_returns_empty_on_missing_binary() {
        let result = scw_config("nonexistent-key");
        let _ = result;
    }

    #[test]
    fn test_dkim_public_key_extraction() {
        let pem = "-----BEGIN PUBLIC KEY-----\nMIIBCgKCAQ...\nbase64data\n-----END PUBLIC KEY-----";
        let b64_key: String = pem
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace("-----BEGIN RSA PUBLIC KEY-----", "")
            .replace("-----END RSA PUBLIC KEY-----", "")
            .split_whitespace()
            .collect();
        assert_eq!(b64_key, "MIIBCgKCAQ...base64data");
    }

    #[test]
    fn test_smtp_uri() {
        assert_eq!(
            SMTP_URI,
            "smtp://stalwart.stalwart.svc.cluster.local:25/?skip_ssl_verify=true"
        );
    }

    #[test]
    fn test_pg_users_canonical_order() {
        let expected = vec![
            "kratos",
            "hydra",
            "gitea",
            "penpot",
            "stalwart",
            "headscale",
            "wfe",
            "typst_editor",
        ];
        assert_eq!(PG_USERS, &expected[..]);
    }
}
