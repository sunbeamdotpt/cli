//! OpenBao KV seeding — init/unseal, idempotent credential generation, VSO auth.

use std::collections::{HashMap, HashSet};

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};

use crate::error::Result;
use crate::kube as k;
use crate::openbao::BaoClient;
use crate::output::{ok, warn};

use super::{
    gen_dkim_key_pair, gen_fernet_key, port_forward, rand_alphanum, rand_token, rand_token_n,
    scw_config, wait_pod_running, delete_resource, GITEA_ADMIN_USER, SMTP_URI,
};

/// Internal result from seed_openbao, used by cmd_seed.
pub struct SeedResult {
    pub creds: HashMap<String, String>,
    pub ob_pod: String,
    pub root_token: String,
}

/// Read-or-create pattern: reads existing KV values, only generates missing ones.
pub async fn get_or_create(
    bao: &BaoClient,
    path: &str,
    fields: &[(&str, &dyn Fn() -> String)],
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
pub async fn seed_openbao() -> Result<Option<SeedResult>> {
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

                // Save to local keystore
                let domain = crate::config::domain();
                let ks = crate::vault_keystore::VaultKeystore {
                    version: 1,
                    domain: domain.to_string(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    root_token: root_token.clone(),
                    unseal_keys_b64: vec![unseal_key.clone()],
                    key_shares: 1,
                    key_threshold: 1,
                };
                crate::vault_keystore::save_keystore(&ks)?;
                ok(&format!("Keys backed up to local keystore at {}", crate::vault_keystore::keystore_path(domain).display()));
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
        let domain = crate::config::domain();

        // Try local keystore first (survives K8s Secret overwrites)
        if crate::vault_keystore::keystore_exists(domain) {
            match crate::vault_keystore::load_keystore(domain) {
                Ok(ks) => {
                    unseal_key = ks.unseal_keys_b64.first().cloned().unwrap_or_default();
                    root_token = ks.root_token.clone();
                    ok("Loaded keys from local keystore.");

                    // Restore K8s Secret if it was wiped
                    let k8s_token = k::kube_get_secret_field("data", "openbao-keys", "root-token").await.unwrap_or_default();
                    if k8s_token.is_empty() && !root_token.is_empty() {
                        warn("K8s Secret openbao-keys is empty — restoring from local keystore.");
                        let mut data = HashMap::new();
                        data.insert("key".to_string(), unseal_key.clone());
                        data.insert("root-token".to_string(), root_token.clone());
                        k::create_secret("data", "openbao-keys", data).await?;
                        ok("Restored openbao-keys from local keystore.");
                    }
                }
                Err(e) => {
                    warn(&format!("Failed to load local keystore: {e}"));
                    // Fall back to K8s Secret
                    if let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await {
                        unseal_key = key;
                    }
                    if let Ok(token) = k::kube_get_secret_field("data", "openbao-keys", "root-token").await {
                        root_token = token;
                    }
                }
            }
        } else {
            // No local keystore — read from K8s Secret and backfill
            if let Ok(key) = k::kube_get_secret_field("data", "openbao-keys", "key").await {
                unseal_key = key;
            }
            if let Ok(token) = k::kube_get_secret_field("data", "openbao-keys", "root-token").await {
                root_token = token;
            }

            // Backfill local keystore if we got keys from the cluster
            if !root_token.is_empty() && !unseal_key.is_empty() {
                let ks = crate::vault_keystore::VaultKeystore {
                    version: 1,
                    domain: domain.to_string(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    root_token: root_token.clone(),
                    unseal_keys_b64: vec![unseal_key.clone()],
                    key_shares: 1,
                    key_threshold: 1,
                };
                if let Err(e) = crate::vault_keystore::save_keystore(&ks) {
                    warn(&format!("Failed to backfill local keystore: {e}"));
                } else {
                    ok(&format!("Backfilled local keystore at {}", crate::vault_keystore::keystore_path(domain).display()));
                }
            }
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
            ("system-secret", &rand_token as &dyn Fn() -> String),
            ("cookie-secret", &rand_token),
            ("pairwise-salt", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let smtp_uri_fn = || SMTP_URI.to_string();
    let cipher_fn = || rand_alphanum(32);
    let kratos = get_or_create(
        &bao,
        "kratos",
        &[
            ("secrets-default", &rand_token as &dyn Fn() -> String),
            ("secrets-cookie", &rand_token),
            ("secrets-cipher", &cipher_fn),
            ("smtp-connection-uri", &smtp_uri_fn),
        ],
        &mut dirty_paths,
    )
    .await?;

    let seaweedfs = get_or_create(
        &bao,
        "seaweedfs",
        &[
            ("access-key", &rand_token as &dyn Fn() -> String),
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
                &gitea_admin_user_fn as &dyn Fn() -> String,
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
            ("oidc-client-id", &hive_local_fn as &dyn Fn() -> String),
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
            ("api-key", &devkey_fn as &dyn Fn() -> String),
            ("api-secret", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let people = get_or_create(
        &bao,
        "people",
        &[("django-secret-key", &rand_token as &dyn Fn() -> String)],
        &mut dirty_paths,
    )
    .await?;

    let login_ui = get_or_create(
        &bao,
        "login-ui",
        &[
            ("cookie-secret", &rand_token as &dyn Fn() -> String),
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
            ("cookie-secret", &rand_token as &dyn Fn() -> String),
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
            ("django-secret-key", &rand_token as &dyn Fn() -> String),
            ("collaboration-secret", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let meet = get_or_create(
        &bao,
        "meet",
        &[
            ("django-secret-key", &rand_token as &dyn Fn() -> String),
            ("application-jwt-secret-key", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let drive = get_or_create(
        &bao,
        "drive",
        &[("django-secret-key", &rand_token as &dyn Fn() -> String)],
        &mut dirty_paths,
    )
    .await?;

    let projects = get_or_create(
        &bao,
        "projects",
        &[("secret-key", &rand_token as &dyn Fn() -> String)],
        &mut dirty_paths,
    )
    .await?;

    let cal_django_fn = || rand_token_n(50);
    let calendars = get_or_create(
        &bao,
        "calendars",
        &[
            ("django-secret-key", &cal_django_fn as &dyn Fn() -> String),
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
            ("django-secret-key", &rand_token as &dyn Fn() -> String),
            ("salt-key", &rand_token),
            ("mda-api-secret", &rand_token),
            (
                "oidc-refresh-token-key",
                &gen_fernet_key as &dyn Fn() -> String,
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
            ("username", &admin_fn as &dyn Fn() -> String),
            ("password", &rand_token),
        ],
        &mut dirty_paths,
    )
    .await?;

    let tuwunel = get_or_create(
        &bao,
        "tuwunel",
        &[
            ("oidc-client-id", &empty_fn as &dyn Fn() -> String),
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
        &[("admin-password", &rand_token as &dyn Fn() -> String)],
        &mut dirty_paths,
    )
    .await?;

    let scw_access_fn = || scw_config("access-key");
    let scw_secret_fn = || scw_config("secret-key");
    let scaleway_s3 = get_or_create(
        &bao,
        "scaleway-s3",
        &[
            ("access-key-id", &scw_access_fn as &dyn Fn() -> String),
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
        ];

        for (path, data) in all_paths {
            if dirty_paths.contains(*path) {
                // Use kv_put for new paths (patch fails with 404 on nonexistent keys).
                // Try patch first (preserves manually-set fields), fall back to put.
                if bao.kv_patch("secret", path, data).await.is_err() {
                    bao.kv_put("secret", path, data).await?;
                }
            }
        }
    }

    // Seed resource server allowed audiences for La Suite external APIs.
    // Combines the static sunbeam-cli client ID with dynamic service client IDs.
    ok("Configuring La Suite resource server audiences...");
    {
        let mut rs_audiences = HashMap::new();
        // sunbeam-cli is always static (OAuth2Client CRD name)
        let mut audiences = vec!["sunbeam-cli".to_string()];
        // Read the messages client ID from the oidc-messages secret if available
        if let Ok(client_id) = crate::kube::kube_get_secret_field("lasuite", "oidc-messages", "CLIENT_ID").await {
            audiences.push(client_id);
        }
        rs_audiences.insert(
            "OIDC_RS_ALLOWED_AUDIENCES".to_string(),
            audiences.join(","),
        );
        bao.kv_put("secret", "drive-rs-audiences", &rs_audiences).await?;
    }

    // Patch gitea admin credentials into secret/sol for Sol's Gitea integration.
    // Uses kv_patch to preserve manually-set keys (matrix-access-token etc.).
    {
        let mut sol_gitea = HashMap::new();
        if let Some(u) = gitea.get("admin-username") {
            sol_gitea.insert("gitea-admin-username".to_string(), u.clone());
        }
        if let Some(p) = gitea.get("admin-password") {
            sol_gitea.insert("gitea-admin-password".to_string(), p.clone());
        }
        if !sol_gitea.is_empty() {
            if bao.kv_patch("secret", "sol", &sol_gitea).await.is_err() {
                bao.kv_put("secret", "sol", &sol_gitea).await?;
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
            "bound_service_account_namespaces": "ory,devtools,storage,lasuite,matrix,media,data,monitoring",
            "policies": "vso-reader",
            "ttl": "1h"
        }),
    )
    .await?;

    // Sol agent policy — read/write access to sol-tokens/* for user impersonation PATs
    ok("Configuring Kubernetes auth for Sol agent...");
    let sol_policy_hcl = concat!(
        "path \"secret/data/sol-tokens/*\" { capabilities = [\"create\", \"read\", \"update\", \"delete\"] }\n",
        "path \"secret/metadata/sol-tokens/*\" { capabilities = [\"read\", \"delete\", \"list\"] }\n",
    );
    bao.write_policy("sol-agent", sol_policy_hcl).await?;

    bao.write(
        "auth/kubernetes/role/sol-agent",
        &serde_json::json!({
            "bound_service_account_names": "default",
            "bound_service_account_namespaces": "matrix",
            "policies": "sol-agent",
            "ttl": "1h"
        }),
    )
    .await?;

    // ── JWT auth for CLI (OIDC via Hydra) ─────────────────────────────
    // Enables `sunbeam vault` commands to authenticate with SSO tokens
    // instead of the root token. Users with `admin: true` in their
    // Kratos metadata_admin get full vault access.
    ok("Configuring JWT/OIDC auth for CLI...");
    let _ = bao.auth_enable("jwt", "jwt").await;

    let domain = crate::config::domain();
    bao.write(
        "auth/jwt/config",
        &serde_json::json!({
            "oidc_discovery_url": format!("https://auth.{domain}/"),
            "default_role": "cli-reader"
        }),
    )
    .await?;

    // Admin role — full access for users with admin: true in JWT
    let admin_policy_hcl = concat!(
        "path \"*\" { capabilities = [\"create\", \"read\", \"update\", \"delete\", \"list\", \"sudo\"] }\n",
    );
    bao.write_policy("cli-admin", admin_policy_hcl).await?;

    bao.write(
        "auth/jwt/role/cli-admin",
        &serde_json::json!({
            "role_type": "jwt",
            "bound_audiences": ["sunbeam-cli"],
            "user_claim": "sub",
            "bound_claims": { "admin": true },
            "policies": ["cli-admin"],
            "ttl": "1h"
        }),
    )
    .await?;

    // Reader role — read-only access for non-admin SSO users
    let cli_reader_hcl = concat!(
        "path \"secret/data/*\" { capabilities = [\"read\"] }\n",
        "path \"secret/metadata/*\" { capabilities = [\"read\", \"list\"] }\n",
        "path \"sys/health\" { capabilities = [\"read\", \"sudo\"] }\n",
        "path \"sys/seal-status\" { capabilities = [\"read\"] }\n",
    );
    bao.write_policy("cli-reader", cli_reader_hcl).await?;

    bao.write(
        "auth/jwt/role/cli-reader",
        &serde_json::json!({
            "role_type": "jwt",
            "bound_audiences": ["sunbeam-cli"],
            "user_claim": "sub",
            "policies": ["cli-reader"],
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
