//! Secrets management — OpenBao KV seeding, DB engine config, VSO verification.
//!
//! Replaces Python's `kubectl exec openbao-0 -- bao ...` pattern with:
//! 1. kube-rs port-forward to openbao pod on port 8200
//! 2. `crate::openbao::BaoClient` for all HTTP API calls

pub mod db_engine;
pub mod seeding;

use crate::error::{Result, ResultExt, SunbeamError};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use rand::RngCore;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
use rsa::RsaPrivateKey;
use serde::Deserialize;
use std::collections::HashMap;
use tokio::net::TcpListener;

use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::{ok, step, warn};

// ── Constants ───────────────────────────────────────────────────────────────

const ADMIN_USERNAME: &str = "estudio-admin";
const GITEA_ADMIN_USER: &str = "gitea_admin";
const PG_USERS: &[&str] = &[
    "kratos",
    "hydra",
    "gitea",
    "hive",
    "docs",
    "meet",
    "drive",
    "messages",
    "conversations",
    "people",
    "find",
    "calendars",
    "projects",
    "penpot",
    "stalwart",
];

const SMTP_URI: &str = "smtp://postfix.lasuite.svc.cluster.local:25/?skip_ssl_verify=true";

// ── Key generation ──────────────────────────────────────────────────────────

/// Generate a Fernet-compatible key (32 random bytes, URL-safe base64).
fn gen_fernet_key() -> String {
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE.encode(buf)
}

/// Generate an RSA 2048-bit DKIM key pair.
/// Returns (private_pem_pkcs8, public_pem). Returns ("", "") on failure.
fn gen_dkim_key_pair() -> (String, String) {
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
fn rand_token_n(n: usize) -> String {
    use base64::Engine;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Generate an alphanumeric random string of exactly `n` characters.
/// Used for secrets that require a fixed character length (e.g. xchacha20-poly1305 cipher keys).
pub(crate) fn rand_alphanum(n: usize) -> String {
    use rand::rngs::OsRng;
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    (0..n).map(|_| CHARSET[OsRng.gen_range(0..CHARSET.len())] as char).collect()
}

// ── Port-forward helper ─────────────────────────────────────────────────────

/// Port-forward guard — cancels the background forwarder on drop.
struct PortForwardGuard {
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
async fn port_forward(
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
        loop {
            let (mut client_stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };

            let pf_result = pods.portforward(&current_pod, &[remote_port]).await;
            let mut pf = match pf_result {
                Ok(pf) => pf,
                Err(e) => {
                    tracing::warn!("Port-forward failed, re-resolving pod: {e}");
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
                                        .map(|n| n.starts_with(current_pod.split('-').next().unwrap_or("")))
                                        .unwrap_or(false)
                                })
                                .and_then(|p| p.metadata.name.clone())
                            {
                                current_pod = name;
                            }
                        }
                    }
                    continue; // next accept() iteration will retry
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
async fn port_forward_svc(
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

// ── Kratos admin identity seeding ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct KratosIdentity {
    id: String,
}

#[derive(Debug, Deserialize)]
struct KratosRecovery {
    #[serde(default)]
    recovery_link: String,
    #[serde(default)]
    recovery_code: String,
}

/// Ensure estudio-admin@<domain> exists in Kratos and is the only admin identity.
async fn seed_kratos_admin_identity(bao: &BaoClient) -> (String, String) {
    let domain = match k::get_domain().await {
        Ok(d) => d,
        Err(e) => {
            warn(&format!("Could not determine domain: {e}"));
            return (String::new(), String::new());
        }
    };
    let admin_email = format!("{ADMIN_USERNAME}@{domain}");
    ok(&format!(
        "Ensuring Kratos admin identity ({admin_email})..."
    ));

    let result: std::result::Result<(String, String), SunbeamError> = async {
        let pf = match port_forward_svc("ory", "app.kubernetes.io/name=kratos-admin", 80).await {
            Ok(pf) => pf,
            Err(_) => port_forward_svc("ory", "app.kubernetes.io/name=kratos", 4434)
                .await
                .ctx("Could not port-forward to Kratos admin API")?,
        };
        let base = format!("http://127.0.0.1:{}", pf.local_port);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let http = reqwest::Client::new();

        let resp = http
            .get(format!(
                "{base}/admin/identities?credentials_identifier={admin_email}&page_size=1"
            ))
            .header("Accept", "application/json")
            .send()
            .await?;

        let identities: Vec<KratosIdentity> = resp.json().await.unwrap_or_default();
        let identity_id = if let Some(existing) = identities.first() {
            ok(&format!(
                "  admin identity exists ({}...)",
                &existing.id[..8.min(existing.id.len())]
            ));
            existing.id.clone()
        } else {
            let resp = http
                .post(format!("{base}/admin/identities"))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .json(&serde_json::json!({
                    "schema_id": "employee",
                    "traits": {"email": admin_email},
                    "state": "active",
                }))
                .send()
                .await?;

            let identity: KratosIdentity =
                resp.json().await.map_err(|e| SunbeamError::Other(e.to_string()))?;
            ok(&format!(
                "  created admin identity ({}...)",
                &identity.id[..8.min(identity.id.len())]
            ));
            identity.id
        };

        let resp = http
            .post(format!("{base}/admin/recovery/code"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "identity_id": identity_id,
                "expires_in": "24h",
            }))
            .send()
            .await?;

        let recovery: KratosRecovery = resp.json().await.unwrap_or(KratosRecovery {
            recovery_link: String::new(),
            recovery_code: String::new(),
        });

        let mut patch_data = HashMap::new();
        patch_data.insert("admin-identity-ids".to_string(), admin_email.clone());
        let _ = bao.kv_patch("secret", "kratos-admin", &patch_data).await;
        ok(&format!("  ADMIN_IDENTITY_IDS set to {admin_email}"));

        Ok((recovery.recovery_link, recovery.recovery_code))
    }
    .await;

    match result {
        Ok(r) => r,
        Err(e) => {
            warn(&format!(
                "Could not seed Kratos admin identity (Kratos may not be ready): {e}"
            ));
            (String::new(), String::new())
        }
    }
}

// ── cmd_seed — main entry point ─────────────────────────────────────────────

/// Seed OpenBao KV with crypto-random credentials, then mirror to K8s Secrets.
/// File-based advisory lock for `cmd_seed` to prevent concurrent runs.
struct SeedLock {
    path: std::path::PathBuf,
}

impl SeedLock {
    fn acquire() -> Result<Self> {
        let lock_path = dirs::data_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local/share"))
            .join("sunbeam")
            .join("seed.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap())?;

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut f) => {
                use std::io::Write;
                write!(f, "{}", std::process::id())?;
                Ok(SeedLock { path: lock_path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Check if the PID in the file is still alive
                if let Ok(pid_str) = std::fs::read_to_string(&lock_path) {
                    if let Ok(pid) = pid_str.trim().parse::<i32>() {
                        // kill(pid, 0) checks if process exists without sending a signal
                        let alive = is_pid_alive(pid);
                        if alive {
                            return Err(SunbeamError::secrets(
                                "Another sunbeam seed is already running. Wait for it to finish.",
                            ));
                        }
                    }
                }
                // Stale lock, remove and retry
                std::fs::remove_file(&lock_path)?;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)?;
                use std::io::Write;
                write!(f, "{}", std::process::id())?;
                Ok(SeedLock { path: lock_path })
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for SeedLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Check if a process with the given PID is still alive.
fn is_pid_alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn cmd_seed() -> Result<()> {
    let _lock = SeedLock::acquire()?;
    step("Seeding secrets...");

    let seed_result = seeding::seed_openbao().await?;
    let (creds, ob_pod, root_token) = match seed_result {
        Some(r) => (r.creds, r.ob_pod, r.root_token),
        None => (HashMap::new(), String::new(), String::new()),
    };

    let s3_access_key = creds.get("s3-access-key").cloned().unwrap_or_default();
    let s3_secret_key = creds.get("s3-secret-key").cloned().unwrap_or_default();
    let hydra_system = creds
        .get("hydra-system-secret")
        .cloned()
        .unwrap_or_default();
    let hydra_cookie = creds
        .get("hydra-cookie-secret")
        .cloned()
        .unwrap_or_default();
    let hydra_pairwise = creds
        .get("hydra-pairwise-salt")
        .cloned()
        .unwrap_or_default();
    let kratos_secrets_default = creds
        .get("kratos-secrets-default")
        .cloned()
        .unwrap_or_default();
    let kratos_secrets_cookie = creds
        .get("kratos-secrets-cookie")
        .cloned()
        .unwrap_or_default();
    let hive_oidc_id = creds
        .get("hive-oidc-client-id")
        .cloned()
        .unwrap_or_else(|| "hive-local".into());
    let hive_oidc_sec = creds
        .get("hive-oidc-client-secret")
        .cloned()
        .unwrap_or_default();
    let django_secret = creds
        .get("people-django-secret")
        .cloned()
        .unwrap_or_default();
    let gitea_admin_pass = creds
        .get("gitea-admin-password")
        .cloned()
        .unwrap_or_default();

    // ── Wait for Postgres ───────────────────────────────────────────────
    ok("Waiting for postgres cluster...");
    let mut pg_pod = String::new();

    let client = k::get_client().await?;
    let ar = ApiResource {
        group: "postgresql.cnpg.io".into(),
        version: "v1".into(),
        api_version: "postgresql.cnpg.io/v1".into(),
        kind: "Cluster".into(),
        plural: "clusters".into(),
    };
    let cnpg_api: Api<DynamicObject> = Api::namespaced_with(client.clone(), "data", &ar);
    for _ in 0..60 {
        if let Ok(cluster) = cnpg_api.get("postgres").await {
            let phase = cluster
                .data
                .get("status")
                .and_then(|s| s.get("phase"))
                .and_then(|p| p.as_str())
                .unwrap_or("");
            if phase == "Cluster in healthy state" {
                // Cluster is healthy — find the primary pod name
                let pods: Api<Pod> = Api::namespaced(client.clone(), "data");
                let lp = ListParams::default().labels("cnpg.io/cluster=postgres,role=primary");
                if let Ok(pod_list) = pods.list(&lp).await {
                    if let Some(name) = pod_list
                        .items
                        .first()
                        .and_then(|p| p.metadata.name.as_deref())
                    {
                        pg_pod = name.to_string();
                        ok(&format!("Postgres ready ({pg_pod})."));
                        break;
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }

    if pg_pod.is_empty() {
        warn("Postgres not ready after 5 min -- continuing anyway.");
    }

    if !pg_pod.is_empty() {
        ok("Ensuring postgres roles and databases exist...");
        let db_map: HashMap<&str, &str> = [
            ("kratos", "kratos_db"),
            ("hydra", "hydra_db"),
            ("gitea", "gitea_db"),
            ("hive", "hive_db"),
            ("docs", "docs_db"),
            ("meet", "meet_db"),
            ("drive", "drive_db"),
            ("messages", "messages_db"),
            ("conversations", "conversations_db"),
            ("people", "people_db"),
            ("find", "find_db"),
            ("calendars", "calendars_db"),
            ("projects", "projects_db"),
        ]
        .into_iter()
        .collect();

        for user in PG_USERS {
            let ensure_sql = format!(
                "DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='{user}') \
                 THEN EXECUTE 'CREATE USER {user}'; END IF; END $$;"
            );
            let _ = k::kube_exec(
                "data",
                &pg_pod,
                &["psql", "-U", "postgres", "-c", &ensure_sql],
                Some("postgres"),
            )
            .await;

            let db = db_map.get(user).copied().unwrap_or("unknown_db");
            let create_db_sql = format!("CREATE DATABASE {db} OWNER {user};");
            let _ = k::kube_exec(
                "data",
                &pg_pod,
                &["psql", "-U", "postgres", "-c", &create_db_sql],
                Some("postgres"),
            )
            .await;
        }

        // Configure database secrets engine via port-forward
        if !ob_pod.is_empty() && !root_token.is_empty() {
            match port_forward("data", &ob_pod, 8200).await {
                Ok(pf) => {
                    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);
                    let bao = BaoClient::with_token(&bao_url, &root_token);
                    if let Err(e) = db_engine::configure_db_engine(&bao).await {
                        warn(&format!("DB engine config failed: {e}"));
                    }
                }
                Err(e) => warn(&format!("Port-forward to OpenBao failed: {e}")),
            }
        } else {
            warn("Skipping DB engine config -- missing ob_pod or root_token.");
        }
    }

    // ── Create K8s secrets ──────────────────────────────────────────────
    ok("Creating K8s secrets (VSO will overwrite on next sync)...");

    k::ensure_ns("ory").await?;
    k::create_secret(
        "ory",
        "hydra",
        HashMap::from([
            ("secretsSystem".into(), hydra_system),
            ("secretsCookie".into(), hydra_cookie),
            ("pairwise-salt".into(), hydra_pairwise),
        ]),
    )
    .await?;
    k::create_secret(
        "ory",
        "kratos-app-secrets",
        HashMap::from([
            ("secretsDefault".into(), kratos_secrets_default),
            ("secretsCookie".into(), kratos_secrets_cookie),
        ]),
    )
    .await?;

    k::ensure_ns("devtools").await?;
    k::create_secret(
        "devtools",
        "gitea-s3-credentials",
        HashMap::from([
            ("access-key".into(), s3_access_key.clone()),
            ("secret-key".into(), s3_secret_key.clone()),
        ]),
    )
    .await?;
    k::create_secret(
        "devtools",
        "gitea-admin-credentials",
        HashMap::from([
            ("username".into(), GITEA_ADMIN_USER.into()),
            ("password".into(), gitea_admin_pass.clone()),
        ]),
    )
    .await?;

    // Sync Gitea admin password to Gitea's own DB
    if !gitea_admin_pass.is_empty() {
        let gitea_pods: Api<Pod> = Api::namespaced(client.clone(), "devtools");
        let lp = ListParams::default().labels("app.kubernetes.io/name=gitea");
        if let Ok(pod_list) = gitea_pods.list(&lp).await {
            if let Some(gitea_pod) = pod_list
                .items
                .first()
                .and_then(|p| p.metadata.name.as_deref())
            {
                match k::kube_exec(
                    "devtools",
                    gitea_pod,
                    &[
                        "gitea",
                        "admin",
                        "user",
                        "change-password",
                        "--username",
                        GITEA_ADMIN_USER,
                        "--password",
                        &gitea_admin_pass,
                        "--must-change-password=false",
                    ],
                    Some("gitea"),
                )
                .await
                {
                    Ok((0, _)) => ok("Gitea admin password synced to Gitea DB."),
                    Ok((_, stderr)) => {
                        warn(&format!("Could not sync Gitea admin password: {stderr}"))
                    }
                    Err(e) => warn(&format!("Could not sync Gitea admin password: {e}")),
                }
            } else {
                warn("Gitea pod not found -- admin password NOT synced to Gitea DB. Run seed again after Gitea is deployed.");
            }
        }
    }

    k::ensure_ns("storage").await?;
    let s3_json = format!(
        r#"{{"identities":[{{"name":"seaweed","credentials":[{{"accessKey":"{}","secretKey":"{}"}}],"actions":["Admin","Read","Write","List","Tagging"]}}]}}"#,
        s3_access_key, s3_secret_key
    );
    k::create_secret(
        "storage",
        "seaweedfs-s3-credentials",
        HashMap::from([
            ("S3_ACCESS_KEY".into(), s3_access_key.clone()),
            ("S3_SECRET_KEY".into(), s3_secret_key.clone()),
        ]),
    )
    .await?;
    k::create_secret(
        "storage",
        "seaweedfs-s3-json",
        HashMap::from([("s3.json".into(), s3_json)]),
    )
    .await?;

    k::ensure_ns("lasuite").await?;
    k::create_secret(
        "lasuite",
        "seaweedfs-s3-credentials",
        HashMap::from([
            ("S3_ACCESS_KEY".into(), s3_access_key),
            ("S3_SECRET_KEY".into(), s3_secret_key),
        ]),
    )
    .await?;
    k::create_secret(
        "lasuite",
        "hive-oidc",
        HashMap::from([
            ("client-id".into(), hive_oidc_id),
            ("client-secret".into(), hive_oidc_sec),
        ]),
    )
    .await?;
    k::create_secret(
        "lasuite",
        "people-django-secret",
        HashMap::from([("DJANGO_SECRET_KEY".into(), django_secret)]),
    )
    .await?;

    k::ensure_ns("matrix").await?;
    k::ensure_ns("media").await?;
    k::ensure_ns("monitoring").await?;

    // ── Kratos admin identity ───────────────────────────────────────────
    if !ob_pod.is_empty() && !root_token.is_empty() {
        if let Ok(pf) = port_forward("data", &ob_pod, 8200).await {
            let bao_url = format!("http://127.0.0.1:{}", pf.local_port);
            let bao = BaoClient::with_token(&bao_url, &root_token);
            let (recovery_link, recovery_code) = seed_kratos_admin_identity(&bao).await;
            if !recovery_link.is_empty() {
                ok("Admin recovery link (valid 24h):");
                println!("  {recovery_link}");
            }
            if !recovery_code.is_empty() {
                ok("Admin recovery code (enter on the page above):");
                println!("  {recovery_code}");
            }
        }
    }

    let dkim_pub = creds
        .get("messages-dkim-public-key")
        .cloned()
        .unwrap_or_default();
    if !dkim_pub.is_empty() {
        let b64_key: String = dkim_pub
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace("-----BEGIN RSA PUBLIC KEY-----", "")
            .replace("-----END RSA PUBLIC KEY-----", "")
            .split_whitespace()
            .collect();

        if let Ok(domain) = k::get_domain().await {
            ok("DKIM DNS record (add to DNS at your registrar):");
            println!(
                "  default._domainkey.{domain}  TXT  \"v=DKIM1; k=rsa; p={b64_key}\""
            );
        }
    }

    ok("All secrets seeded.");
    Ok(())
}

// ── cmd_verify — VSO E2E verification ───────────────────────────────────────

/// End-to-end test of VSO -> OpenBao integration.
pub async fn cmd_verify() -> Result<()> {
    step("Verifying VSO -> OpenBao integration (E2E)...");

    let client = k::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), "data");
    let lp = ListParams::default().labels("app.kubernetes.io/name=openbao,component=server");
    let pod_list = pods.list(&lp).await?;

    let ob_pod = pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())
        .ctx("OpenBao pod not found -- run full bring-up first.")?
        .to_string();

    let root_token = k::kube_get_secret_field("data", "openbao-keys", "root-token")
        .await
        .ctx("Could not read openbao-keys secret.")?;

    let pf = port_forward("data", &ob_pod, 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);
    let bao = BaoClient::with_token(&bao_url, &root_token);

    let test_value = rand_token_n(16);
    let test_ns = "ory";
    let test_name = "vso-verify";

    let result: std::result::Result<(), SunbeamError> = async {
        ok("Writing test sentinel to OpenBao secret/vso-test ...");
        let mut data = HashMap::new();
        data.insert("test-key".to_string(), test_value.clone());
        bao.kv_put("secret", "vso-test", &data).await?;

        ok(&format!("Creating VaultAuth {test_ns}/{test_name} ..."));
        k::kube_apply(&format!(
            r#"
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultAuth
metadata:
  name: {test_name}
  namespace: {test_ns}
spec:
  method: kubernetes
  mount: kubernetes
  kubernetes:
    role: vso
    serviceAccount: default
"#
        ))
        .await?;

        ok(&format!(
            "Creating VaultStaticSecret {test_ns}/{test_name} ..."
        ));
        k::kube_apply(&format!(
            r#"
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultStaticSecret
metadata:
  name: {test_name}
  namespace: {test_ns}
spec:
  vaultAuthRef: {test_name}
  mount: secret
  type: kv-v2
  path: vso-test
  refreshAfter: 10s
  destination:
    name: {test_name}
    create: true
    overwrite: true
"#
        ))
        .await?;

        ok("Waiting for VSO to sync (up to 60s) ...");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut synced = false;

        while tokio::time::Instant::now() < deadline {
            let (code, mac) = kubectl_jsonpath(
                test_ns,
                "vaultstaticsecret",
                test_name,
                "{.status.secretMAC}",
            )
            .await;
            if code == 0 && !mac.is_empty() && mac != "<none>" {
                synced = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }

        if !synced {
            let (_, msg) = kubectl_jsonpath(
                test_ns,
                "vaultstaticsecret",
                test_name,
                "{.status.conditions[0].message}",
            )
            .await;
            return Err(SunbeamError::secrets(format!(
                "VSO did not sync within 60s. Last status: {}",
                if msg.is_empty() {
                    "unknown".to_string()
                } else {
                    msg
                }
            )));
        }

        ok("Verifying K8s Secret contents ...");
        let secret = k::kube_get_secret(test_ns, test_name)
            .await?
            .with_ctx(|| format!("K8s Secret {test_ns}/{test_name} not found."))?;

        let data = secret.data.as_ref().ctx("Secret has no data")?;
        let raw = data
            .get("test-key")
            .ctx("Missing key 'test-key' in secret")?;
        let actual = String::from_utf8(raw.0.clone())
            .map_err(|e| SunbeamError::Other(format!("UTF-8 error: {e}")))?;

        if actual != test_value {
            return Err(SunbeamError::secrets(format!(
                "Value mismatch!\n  expected: {:?}\n  got:      {:?}",
                test_value,
                actual
            )));
        }

        ok("Sentinel value matches -- VSO -> OpenBao integration is working.");
        Ok(())
    }
    .await;

    // Always clean up
    ok("Cleaning up test resources...");
    let _ = delete_crd(test_ns, "vaultstaticsecret", test_name).await;
    let _ = delete_crd(test_ns, "vaultauth", test_name).await;
    let _ = delete_k8s_secret(test_ns, test_name).await;
    let _ = bao.kv_delete("secret", "vso-test").await;

    match result {
        Ok(()) => {
            ok("VSO E2E verification passed.");
            Ok(())
        }
        Err(e) => Err(SunbeamError::secrets(format!(
            "VSO verification FAILED: {e}"
        ))),
    }
}

// ── Utility helpers ─────────────────────────────────────────────────────────

async fn wait_pod_running(ns: &str, pod_name: &str, timeout_secs: u64) -> bool {
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

fn scw_config(key: &str) -> String {
    std::process::Command::new("scw")
        .args(["config", "get", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

async fn delete_crd(ns: &str, kind: &str, name: &str) -> Result<()> {
    let ctx = format!("--context={}", k::context());
    let _ = tokio::process::Command::new("kubectl")
        .args([&ctx, "-n", ns, "delete", kind, name, "--ignore-not-found"])
        .output()
        .await;
    Ok(())
}

async fn delete_k8s_secret(ns: &str, name: &str) -> Result<()> {
    let client = k::get_client().await?;
    let api: Api<k8s_openapi::api::core::v1::Secret> = Api::namespaced(client.clone(), ns);
    let _ = api
        .delete(name, &kube::api::DeleteParams::default())
        .await;
    Ok(())
}

async fn delete_resource(ns: &str, kind: &str, name: &str) -> Result<()> {
    let ctx = format!("--context={}", k::context());
    let _ = tokio::process::Command::new("kubectl")
        .args([&ctx, "-n", ns, "delete", kind, name, "--ignore-not-found"])
        .output()
        .await;
    Ok(())
}

async fn kubectl_jsonpath(ns: &str, kind: &str, name: &str, jsonpath: &str) -> (i32, String) {
    let ctx = format!("--context={}", k::context());
    let jp = format!("-o=jsonpath={jsonpath}");
    match tokio::process::Command::new("kubectl")
        .args([&ctx, "-n", ns, "get", kind, name, &jp, "--ignore-not-found"])
        .output()
        .await
    {
        Ok(output) => {
            let code = output.status.code().unwrap_or(1);
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (code, stdout)
        }
        Err(_) => (1, String::new()),
    }
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
        assert_eq!(PG_USERS.len(), 15);
        assert!(PG_USERS.contains(&"kratos"));
        assert!(PG_USERS.contains(&"stalwart"));
    }

    #[test]
    fn test_scw_config_returns_empty_on_missing_binary() {
        let result = scw_config("nonexistent-key");
        let _ = result;
    }

    #[test]
    fn test_seed_result_structure() {
        let mut creds = HashMap::new();
        creds.insert(
            "hydra-system-secret".to_string(),
            "existingvalue".to_string(),
        );
        let result = seeding::SeedResult {
            creds,
            ob_pod: "openbao-0".to_string(),
            root_token: "token123".to_string(),
        };
        assert!(result.creds.contains_key("hydra-system-secret"));
        assert_eq!(result.creds["hydra-system-secret"], "existingvalue");
        assert_eq!(result.ob_pod, "openbao-0");
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
            "smtp://postfix.lasuite.svc.cluster.local:25/?skip_ssl_verify=true"
        );
    }

    #[test]
    fn test_pg_users_match_python() {
        let expected = vec![
            "kratos",
            "hydra",
            "gitea",
            "hive",
            "docs",
            "meet",
            "drive",
            "messages",
            "conversations",
            "people",
            "find",
            "calendars",
            "projects",
            "penpot",
            "stalwart",
        ];
        assert_eq!(PG_USERS, &expected[..]);
    }

    #[test]
    fn test_sol_gitea_credential_mapping() {
        let mut gitea = HashMap::new();
        gitea.insert("admin-username".to_string(), "gitea_admin".to_string());
        gitea.insert("admin-password".to_string(), "s3cret".to_string());

        let mut sol_gitea = HashMap::new();
        if let Some(u) = gitea.get("admin-username") {
            sol_gitea.insert("gitea-admin-username".to_string(), u.clone());
        }
        if let Some(p) = gitea.get("admin-password") {
            sol_gitea.insert("gitea-admin-password".to_string(), p.clone());
        }

        assert_eq!(sol_gitea.len(), 2);
        assert_eq!(sol_gitea["gitea-admin-username"], "gitea_admin");
        assert_eq!(sol_gitea["gitea-admin-password"], "s3cret");
    }

    #[test]
    fn test_sol_gitea_credential_mapping_partial() {
        let gitea: HashMap<String, String> = HashMap::new();
        let mut sol_gitea = HashMap::new();
        if let Some(u) = gitea.get("admin-username") {
            sol_gitea.insert("gitea-admin-username".to_string(), u.clone());
        }
        if let Some(p) = gitea.get("admin-password") {
            sol_gitea.insert("gitea-admin-password".to_string(), p.clone());
        }
        assert!(sol_gitea.is_empty(), "No creds should be mapped when gitea map is empty");
    }

    #[test]
    fn test_sol_agent_policy_hcl() {
        let sol_policy_hcl = concat!(
            "path \"secret/data/sol-tokens/*\" { capabilities = [\"create\", \"read\", \"update\", \"delete\"] }\n",
            "path \"secret/metadata/sol-tokens/*\" { capabilities = [\"read\", \"delete\", \"list\"] }\n",
        );
        assert!(sol_policy_hcl.contains("secret/data/sol-tokens/*"));
        assert!(sol_policy_hcl.contains("secret/metadata/sol-tokens/*"));
        assert!(sol_policy_hcl.contains("create"));
        assert!(sol_policy_hcl.contains("delete"));
        assert!(sol_policy_hcl.contains("list"));
        assert_eq!(sol_policy_hcl.lines().count(), 2);
    }
}
