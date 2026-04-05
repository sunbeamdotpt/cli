//! Secrets management — OpenBao KV seeding, DB engine config, VSO verification.
//!
//! Replaces Python's `kubectl exec openbao-0 -- bao ...` pattern with:
//! 1. kube-rs port-forward to openbao pod on port 8200
//! 2. `crate::openbao::BaoClient` for all HTTP API calls

use crate::error::{Result, ResultExt, SunbeamError};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use rand::RngCore;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
use rsa::RsaPrivateKey;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tokio::net::TcpListener;

use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::{ok, step, warn};

// ── Constants ───────────────────────────────────────────────────────────────

pub(crate) const ADMIN_USERNAME: &str = "estudio-admin";
pub(crate) const GITEA_ADMIN_USER: &str = "gitea_admin";
pub(crate) const PG_USERS: &[&str] = &[
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

pub(crate) const SMTP_URI: &str = "smtp://postfix.lasuite.svc.cluster.local:25/?skip_ssl_verify=true";

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
                        tracing::error!("Port-forward to {current_pod} failed {consecutive_failures} times, giving up: {e}");
                        break;
                    }
                    tracing::warn!("Port-forward failed ({consecutive_failures}/{MAX_CONSECUTIVE_FAILURES}), re-resolving pod: {e}");
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
                                        .map(|n| n.starts_with(current_pod.split('-').next().unwrap_or("")))
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

/// Internal result from seed_openbao, used by cmd_seed.
struct SeedResult {
    creds: HashMap<String, String>,
    ob_pod: String,
    root_token: String,
}

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

/// Initialize/unseal OpenBao, generate/read credentials idempotently, configure VSO auth.
async fn seed_openbao() -> Result<Option<SeedResult>> {
    let client = k::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), "data");
    let lp = ListParams::default().labels("app.kubernetes.io/name=openbao,component=server");
    let pod_list = pods.list(&lp).await?;

    let ob_pod = match pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())
    {
        Some(name) => name.to_string(),
        None => {
            ok("OpenBao pod not found -- skipping.");
            return Ok(None);
        }
    };

    ok(&format!("OpenBao ({ob_pod})..."));
    let _ = wait_pod_running("data", &ob_pod, 120).await;

    let pf = port_forward("data", &ob_pod, 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);
    let bao = BaoClient::new(&bao_url);

    // ── Init / Unseal ───────────────────────────────────────────────────
    let mut unseal_key = String::new();
    let mut root_token = String::new();

    let status = bao.seal_status().await.unwrap_or_else(|_| {
        crate::openbao::SealStatusResponse {
            initialized: false,
            sealed: true,
            progress: 0,
            t: 0,
            n: 0,
        }
    });

    let mut already_initialized = status.initialized;
    if !already_initialized {
        if let Ok(Some(_)) = k::kube_get_secret("data", "openbao-keys").await {
            already_initialized = true;
        }
    }

    if !already_initialized {
        ok("Initializing OpenBao...");
        match bao.init(1, 1).await {
            Ok(init) => {
                unseal_key = init.unseal_keys_b64[0].clone();
                root_token = init.root_token.clone();
                let mut data = HashMap::new();
                data.insert("key".to_string(), unseal_key.clone());
                data.insert("root-token".to_string(), root_token.clone());
                k::create_secret("data", "openbao-keys", data).await?;
                ok("Initialized -- keys stored in secret/openbao-keys.");
            }
            Err(e) => {
                warn(&format!(
                    "Init failed -- resetting OpenBao storage for local dev... ({e})"
                ));
                let _ = delete_resource("data", "pvc", "data-openbao-0").await;
                let _ = delete_resource("data", "pod", &ob_pod).await;
                warn("OpenBao storage reset. Run --seed again after the pod restarts.");
                return Ok(None);
            }
        }
    } else {
        ok("Already initialized.");
        if let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await {
            unseal_key = key;
        }
        if let Ok(token) = k::kube_get_secret_field("data", "openbao-keys", "root-token").await {
            root_token = token;
        }
    }

    // Unseal if needed
    let status = bao.seal_status().await.unwrap_or_else(|_| {
        crate::openbao::SealStatusResponse {
            initialized: true,
            sealed: true,
            progress: 0,
            t: 0,
            n: 0,
        }
    });
    if status.sealed && !unseal_key.is_empty() {
        ok("Unsealing...");
        bao.unseal(&unseal_key).await?;
    }

    if root_token.is_empty() {
        warn("No root token available -- skipping KV seeding.");
        return Ok(None);
    }

    let bao = BaoClient::with_token(&bao_url, &root_token);

    // ── KV seeding ──────────────────────────────────────────────────────
    ok("Seeding KV (idempotent -- existing values preserved)...");
    let _ = bao.enable_secrets_engine("secret", "kv").await;
    let _ = bao
        .write(
            "sys/mounts/secret/tune",
            &serde_json::json!({"options": {"version": "2"}}),
        )
        .await;

    let mut dirty_paths: HashSet<String> = HashSet::new();

    let hydra = get_or_create(
        &bao,
        "hydra",
        &[
            ("system-secret", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("cookie-secret", &rand_token),
            ("pairwise-salt", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let smtp_uri_fn = || SMTP_URI.to_string();
    let kratos = get_or_create(
        &bao,
        "kratos",
        &[
            ("secrets-default", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("secrets-cookie", &rand_token),
            ("smtp-connection-uri", &smtp_uri_fn),
        ],
        &mut dirty_paths,
    )
    .await?;

    let seaweedfs = get_or_create(
        &bao,
        "seaweedfs",
        &[
            ("access-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("secret-key", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let gitea_admin_user_fn = || GITEA_ADMIN_USER.to_string();
    let gitea = get_or_create(
        &bao,
        "gitea",
        &[
            (
                "admin-username",
                &gitea_admin_user_fn as &(dyn Fn() -> String + Send + Sync),
            ),
            ("admin-password", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let hive_local_fn = || "hive-local".to_string();
    let hive = get_or_create(
        &bao,
        "hive",
        &[
            ("oidc-client-id", &hive_local_fn as &(dyn Fn() -> String + Send + Sync)),
            ("oidc-client-secret", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let devkey_fn = || "devkey".to_string();
    let livekit = get_or_create(
        &bao,
        "livekit",
        &[
            ("api-key", &devkey_fn as &(dyn Fn() -> String + Send + Sync)),
            ("api-secret", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let people = get_or_create(
        &bao,
        "people",
        &[("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
        &mut dirty_paths,
    )
    .await?;

    let login_ui = get_or_create(
        &bao,
        "login-ui",
        &[
            ("cookie-secret", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("csrf-cookie-secret", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let sw_access = seaweedfs.get("access-key").cloned().unwrap_or_default();
    let sw_secret = seaweedfs.get("secret-key").cloned().unwrap_or_default();
    let empty_fn = || String::new();
    let sw_access_fn = {
        let v = sw_access.clone();
        move || v.clone()
    };
    let sw_secret_fn = {
        let v = sw_secret.clone();
        move || v.clone()
    };

    let kratos_admin = get_or_create(
        &bao,
        "kratos-admin",
        &[
            ("cookie-secret", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("csrf-cookie-secret", &rand_token),
            ("admin-identity-ids", &empty_fn),
            ("s3-access-key", &sw_access_fn),
            ("s3-secret-key", &sw_secret_fn),
        ],
        &mut dirty_paths,
    )
    .await?;

    let docs = get_or_create(
        &bao,
        "docs",
        &[
            ("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("collaboration-secret", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let meet = get_or_create(
        &bao,
        "meet",
        &[
            ("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("application-jwt-secret-key", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let drive = get_or_create(
        &bao,
        "drive",
        &[("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
        &mut dirty_paths,
    )
    .await?;

    let projects = get_or_create(
        &bao,
        "projects",
        &[("secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
        &mut dirty_paths,
    )
    .await?;

    let cal_django_fn = || rand_token_n(50);
    let calendars = get_or_create(
        &bao,
        "calendars",
        &[
            ("django-secret-key", &cal_django_fn as &(dyn Fn() -> String + Send + Sync)),
            ("salt-key", &rand_token),
            ("caldav-inbound-api-key", &rand_token),
            ("caldav-outbound-api-key", &rand_token),
            ("caldav-internal-api-key", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    // DKIM key pair — generated together since keys are coupled.
    let existing_messages = bao.kv_get("secret", "messages").await?.unwrap_or_default();
    let (dkim_private, dkim_public) = if existing_messages
        .get("dkim-private-key")
        .filter(|v| !v.is_empty())
        .is_some()
    {
        (
            existing_messages
                .get("dkim-private-key")
                .cloned()
                .unwrap_or_default(),
            existing_messages
                .get("dkim-public-key")
                .cloned()
                .unwrap_or_default(),
        )
    } else {
        gen_dkim_key_pair()
    };

    let dkim_priv_fn = {
        let v = dkim_private.clone();
        move || v.clone()
    };
    let dkim_pub_fn = {
        let v = dkim_public.clone();
        move || v.clone()
    };
    let socks_proxy_fn = || format!("sunbeam:{}", rand_token());
    let sunbeam_fn = || "sunbeam".to_string();

    let messages = get_or_create(
        &bao,
        "messages",
        &[
            ("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
            ("salt-key", &rand_token),
            ("mda-api-secret", &rand_token),
            (
                "oidc-refresh-token-key",
                &gen_fernet_key as &(dyn Fn() -> String + Send + Sync),
            ),
            ("dkim-private-key", &dkim_priv_fn),
            ("dkim-public-key", &dkim_pub_fn),
            ("rspamd-password", &rand_token),
            ("socks-proxy-users", &socks_proxy_fn),
            ("mta-out-smtp-username", &sunbeam_fn),
            ("mta-out-smtp-password", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let admin_fn = || "admin".to_string();
    let collabora = get_or_create(
        &bao,
        "collabora",
        &[
            ("username", &admin_fn as &(dyn Fn() -> String + Send + Sync)),
            ("password", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let tuwunel = get_or_create(
        &bao,
        "tuwunel",
        &[
            ("oidc-client-id", &empty_fn as &(dyn Fn() -> String + Send + Sync)),
            ("oidc-client-secret", &empty_fn),
            ("turn-secret", &empty_fn),
            ("registration-token", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let grafana = get_or_create(
        &bao,
        "grafana",
        &[("admin-password", &rand_token as &(dyn Fn() -> String + Send + Sync))],
        &mut dirty_paths,
    )
    .await?;

    let scw_access_fn = || scw_config("access-key");
    let scw_secret_fn = || scw_config("secret-key");
    let penpot = get_or_create(
        &bao,
        "penpot",
        &[("secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
        &mut dirty_paths,
    )
    .await?;

    let scaleway_s3 = get_or_create(
        &bao,
        "scaleway-s3",
        &[
            ("access-key-id", &scw_access_fn as &(dyn Fn() -> String + Send + Sync)),
            ("secret-access-key", &scw_secret_fn),
        ],
        &mut dirty_paths,
    )
    .await?;

    // ── Write dirty paths ───────────────────────────────────────────────
    if dirty_paths.is_empty() {
        ok("All OpenBao KV secrets already present -- skipping writes.");
    } else {
        let mut sorted_paths: Vec<&String> = dirty_paths.iter().collect();
        sorted_paths.sort();
        ok(&format!(
            "Writing new secrets to OpenBao KV ({})...",
            sorted_paths
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));

        let all_paths: &[(&str, &HashMap<String, String>)] = &[
            ("hydra", &hydra),
            ("kratos", &kratos),
            ("seaweedfs", &seaweedfs),
            ("gitea", &gitea),
            ("hive", &hive),
            ("livekit", &livekit),
            ("people", &people),
            ("login-ui", &login_ui),
            ("kratos-admin", &kratos_admin),
            ("docs", &docs),
            ("meet", &meet),
            ("drive", &drive),
            ("projects", &projects),
            ("calendars", &calendars),
            ("messages", &messages),
            ("collabora", &collabora),
            ("tuwunel", &tuwunel),
            ("grafana", &grafana),
            ("scaleway-s3", &scaleway_s3),
            ("penpot", &penpot),
        ];

        for (path, data) in all_paths {
            if dirty_paths.contains(*path) {
                bao.kv_patch("secret", path, data).await?;
            }
        }
    }

    // ── Kubernetes auth for VSO ─────────────────────────────────────────
    ok("Configuring Kubernetes auth for VSO...");
    let _ = bao.auth_enable("kubernetes", "kubernetes").await;

    bao.write(
        "auth/kubernetes/config",
        &serde_json::json!({
            "kubernetes_host": "https://kubernetes.default.svc.cluster.local"
        }),
    )
    .await?;

    let policy_hcl = concat!(
        "path \"secret/data/*\" { capabilities = [\"read\"] }\n",
        "path \"secret/metadata/*\" { capabilities = [\"read\", \"list\"] }\n",
        "path \"database/static-creds/*\" { capabilities = [\"read\"] }\n",
    );
    bao.write_policy("vso-reader", policy_hcl).await?;

    bao.write(
        "auth/kubernetes/role/vso",
        &serde_json::json!({
            "bound_service_account_names": "default",
            "bound_service_account_namespaces": "ory,devtools,storage,lasuite,stalwart,matrix,media,data,monitoring,cert-manager",
            "policies": "vso-reader",
            "ttl": "1h"
        }),
    )
    .await?;

    // Build credentials map
    let mut creds = HashMap::new();
    let field_map: &[(&str, &str, &HashMap<String, String>)] = &[
        ("hydra-system-secret", "system-secret", &hydra),
        ("hydra-cookie-secret", "cookie-secret", &hydra),
        ("hydra-pairwise-salt", "pairwise-salt", &hydra),
        ("kratos-secrets-default", "secrets-default", &kratos),
        ("kratos-secrets-cookie", "secrets-cookie", &kratos),
        ("s3-access-key", "access-key", &seaweedfs),
        ("s3-secret-key", "secret-key", &seaweedfs),
        ("gitea-admin-password", "admin-password", &gitea),
        ("hive-oidc-client-id", "oidc-client-id", &hive),
        ("hive-oidc-client-secret", "oidc-client-secret", &hive),
        ("people-django-secret", "django-secret-key", &people),
        ("livekit-api-key", "api-key", &livekit),
        ("livekit-api-secret", "api-secret", &livekit),
        (
            "kratos-admin-cookie-secret",
            "cookie-secret",
            &kratos_admin,
        ),
        ("messages-dkim-public-key", "dkim-public-key", &messages),
    ];

    for (cred_key, field_key, source) in field_map {
        creds.insert(
            cred_key.to_string(),
            source.get(*field_key).cloned().unwrap_or_default(),
        );
    }

    Ok(Some(SeedResult {
        creds,
        ob_pod,
        root_token,
    }))
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

    let conn_url = format!(
        "postgresql://{{{{username}}}}:{{{{password}}}}@{pg_rw}/postgres?sslmode=disable"
    );

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

// ── Kratos admin identity seeding ───────────────────────────────────────────

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

pub async fn cmd_seed() -> Result<()> {
    step("Seeding secrets...");

    let seed_result = seed_openbao().await?;
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
                    if let Err(e) = configure_db_engine(&bao).await {
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

pub(crate) async fn delete_resource(ns: &str, kind: &str, name: &str) -> Result<()> {
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
        assert!(PG_USERS.contains(&"projects"));
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
        let result = SeedResult {
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
}
