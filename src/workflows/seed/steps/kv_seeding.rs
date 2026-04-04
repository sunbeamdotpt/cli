//! KV secret seeding steps: generate/read all credentials, write dirty paths,
//! configure Kubernetes auth for VSO.
//!
//! Data-struct-agnostic — reads JSON fields directly for cross-workflow reuse.

use std::collections::{HashMap, HashSet};

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::openbao::BaoClient;
use crate::output::{ok, warn};
use crate::secrets::{
    self, gen_dkim_key_pair, gen_fernet_key, rand_token, rand_token_n, scw_config,
    GITEA_ADMIN_USER, SMTP_URI,
};

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

fn json_bool(data: &serde_json::Value, key: &str) -> bool {
    data.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn json_str(data: &serde_json::Value, key: &str) -> Option<String> {
    data.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

// ── SeedAllKVPaths ──────────────────────────────────────────────────────────

/// Single step that runs the get_or_create loop for all 19 services.
/// Sets `creds` and `dirty_paths` in data.
///
/// Reads: `skip_seed`, `ob_pod`, `root_token`
/// Writes: `creds`, `dirty_paths`
#[derive(Default)]
pub struct SeedAllKVPaths;

#[async_trait::async_trait]
impl StepBody for SeedAllKVPaths {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if json_bool(data, "skip_seed") {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match json_str(data, "ob_pod") {
            Some(p) => p,
            None => {
                warn("No ob_pod set -- skipping KV seeding.");
                return Ok(ExecutionResult::next());
            }
        };
        let root_token = match json_str(data, "root_token") {
            Some(t) => t,
            None => {
                warn("No root_token set -- skipping KV seeding.");
                return Ok(ExecutionResult::next());
            }
        };

        let pf = secrets::port_forward("data", &ob_pod, 8200).await
            .map_err(|e| step_err(e.to_string()))?;
        let bao = BaoClient::with_token(
            &format!("http://127.0.0.1:{}", pf.local_port),
            &root_token,
        );

        let mut dirty_paths: HashSet<String> = HashSet::new();

        let hydra = secrets::get_or_create(
            &bao, "hydra",
            &[
                ("system-secret", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("cookie-secret", &rand_token),
                ("pairwise-salt", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let smtp_uri_fn = || SMTP_URI.to_string();
        let kratos = secrets::get_or_create(
            &bao, "kratos",
            &[
                ("secrets-default", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("secrets-cookie", &rand_token),
                ("smtp-connection-uri", &smtp_uri_fn),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let seaweedfs = secrets::get_or_create(
            &bao, "seaweedfs",
            &[
                ("access-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("secret-key", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let gitea_admin_user_fn = || GITEA_ADMIN_USER.to_string();
        let gitea = secrets::get_or_create(
            &bao, "gitea",
            &[
                ("admin-username", &gitea_admin_user_fn as &(dyn Fn() -> String + Send + Sync)),
                ("admin-password", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let hive_local_fn = || "hive-local".to_string();
        let hive = secrets::get_or_create(
            &bao, "hive",
            &[
                ("oidc-client-id", &hive_local_fn as &(dyn Fn() -> String + Send + Sync)),
                ("oidc-client-secret", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let devkey_fn = || "devkey".to_string();
        let livekit = secrets::get_or_create(
            &bao, "livekit",
            &[
                ("api-key", &devkey_fn as &(dyn Fn() -> String + Send + Sync)),
                ("api-secret", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let people = secrets::get_or_create(
            &bao, "people",
            &[("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let login_ui = secrets::get_or_create(
            &bao, "login-ui",
            &[
                ("cookie-secret", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("csrf-cookie-secret", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let sw_access = seaweedfs.get("access-key").cloned().unwrap_or_default();
        let sw_secret = seaweedfs.get("secret-key").cloned().unwrap_or_default();
        let empty_fn = || String::new();
        let sw_access_fn = { let v = sw_access.clone(); move || v.clone() };
        let sw_secret_fn = { let v = sw_secret.clone(); move || v.clone() };

        let kratos_admin = secrets::get_or_create(
            &bao, "kratos-admin",
            &[
                ("cookie-secret", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("csrf-cookie-secret", &rand_token),
                ("admin-identity-ids", &empty_fn),
                ("s3-access-key", &sw_access_fn),
                ("s3-secret-key", &sw_secret_fn),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let docs = secrets::get_or_create(
            &bao, "docs",
            &[
                ("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("collaboration-secret", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let meet = secrets::get_or_create(
            &bao, "meet",
            &[
                ("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("application-jwt-secret-key", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let drive = secrets::get_or_create(
            &bao, "drive",
            &[("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let projects = secrets::get_or_create(
            &bao, "projects",
            &[("secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync))],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let cal_django_fn = || rand_token_n(50);
        let calendars = secrets::get_or_create(
            &bao, "calendars",
            &[
                ("django-secret-key", &cal_django_fn as &(dyn Fn() -> String + Send + Sync)),
                ("salt-key", &rand_token),
                ("caldav-inbound-api-key", &rand_token),
                ("caldav-outbound-api-key", &rand_token),
                ("caldav-internal-api-key", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        // DKIM key pair
        let existing_messages = bao.kv_get("secret", "messages").await
            .map_err(|e| step_err(e.to_string()))?
            .unwrap_or_default();
        let (dkim_private, dkim_public) = if existing_messages
            .get("dkim-private-key").filter(|v| !v.is_empty()).is_some()
        {
            (
                existing_messages.get("dkim-private-key").cloned().unwrap_or_default(),
                existing_messages.get("dkim-public-key").cloned().unwrap_or_default(),
            )
        } else {
            gen_dkim_key_pair()
        };

        let dkim_priv_fn = { let v = dkim_private.clone(); move || v.clone() };
        let dkim_pub_fn = { let v = dkim_public.clone(); move || v.clone() };
        let socks_proxy_fn = || format!("sunbeam:{}", rand_token());
        let sunbeam_fn = || "sunbeam".to_string();

        let messages = secrets::get_or_create(
            &bao, "messages",
            &[
                ("django-secret-key", &rand_token as &(dyn Fn() -> String + Send + Sync)),
                ("salt-key", &rand_token),
                ("mda-api-secret", &rand_token),
                ("oidc-refresh-token-key", &gen_fernet_key as &(dyn Fn() -> String + Send + Sync)),
                ("dkim-private-key", &dkim_priv_fn),
                ("dkim-public-key", &dkim_pub_fn),
                ("rspamd-password", &rand_token),
                ("socks-proxy-users", &socks_proxy_fn),
                ("mta-out-smtp-username", &sunbeam_fn),
                ("mta-out-smtp-password", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let admin_fn = || "admin".to_string();
        let collabora = secrets::get_or_create(
            &bao, "collabora",
            &[
                ("username", &admin_fn as &(dyn Fn() -> String + Send + Sync)),
                ("password", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let tuwunel = secrets::get_or_create(
            &bao, "tuwunel",
            &[
                ("oidc-client-id", &empty_fn as &(dyn Fn() -> String + Send + Sync)),
                ("oidc-client-secret", &empty_fn),
                ("turn-secret", &empty_fn),
                ("registration-token", &rand_token),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let grafana = secrets::get_or_create(
            &bao, "grafana",
            &[("admin-password", &rand_token as &(dyn Fn() -> String + Send + Sync))],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

        let scw_access_fn = || scw_config("access-key");
        let scw_secret_fn = || scw_config("secret-key");
        let scaleway_s3 = secrets::get_or_create(
            &bao, "scaleway-s3",
            &[
                ("access-key-id", &scw_access_fn as &(dyn Fn() -> String + Send + Sync)),
                ("secret-access-key", &scw_secret_fn),
            ],
            &mut dirty_paths,
        ).await.map_err(|e| step_err(e.to_string()))?;

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
            ("kratos-admin-cookie-secret", "cookie-secret", &kratos_admin),
            ("messages-dkim-public-key", "dkim-public-key", &messages),
        ];

        for (cred_key, field_key, source) in field_map {
            creds.insert(cred_key.to_string(), source.get(*field_key).cloned().unwrap_or_default());
        }

        // Store per-path data for WriteDirtyKVPaths
        let all_paths: &[(&str, &HashMap<String, String>)] = &[
            ("hydra", &hydra), ("kratos", &kratos), ("seaweedfs", &seaweedfs),
            ("gitea", &gitea), ("hive", &hive), ("livekit", &livekit),
            ("people", &people), ("login-ui", &login_ui), ("kratos-admin", &kratos_admin),
            ("docs", &docs), ("meet", &meet), ("drive", &drive),
            ("projects", &projects), ("calendars", &calendars), ("messages", &messages),
            ("collabora", &collabora), ("tuwunel", &tuwunel), ("grafana", &grafana),
            ("scaleway-s3", &scaleway_s3),
        ];

        for (path, data) in all_paths {
            let json = serde_json::to_string(data).map_err(|e| step_err(e.to_string()))?;
            creds.insert(format!("kv_data/{path}"), json);
        }

        let dirty_vec: Vec<String> = dirty_paths.into_iter().collect();

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({
            "creds": creds,
            "dirty_paths": dirty_vec,
        }));
        Ok(result)
    }
}

// ── WriteDirtyKVPaths ───────────────────────────────────────────────────────

/// Write all modified KV paths to OpenBao.
///
/// Reads: `skip_seed`, `ob_pod`, `root_token`, `dirty_paths`, `creds`
#[derive(Default)]
pub struct WriteDirtyKVPaths;

#[async_trait::async_trait]
impl StepBody for WriteDirtyKVPaths {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if json_bool(data, "skip_seed") {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match json_str(data, "ob_pod") {
            Some(p) => p,
            None => return Ok(ExecutionResult::next()),
        };
        let root_token = match json_str(data, "root_token") {
            Some(t) => t,
            None => return Ok(ExecutionResult::next()),
        };

        let dirty_paths: Vec<String> = data.get("dirty_paths")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if dirty_paths.is_empty() {
            ok("All OpenBao KV secrets already present -- skipping writes.");
            return Ok(ExecutionResult::next());
        }

        let mut sorted_paths = dirty_paths.clone();
        sorted_paths.sort();
        ok(&format!("Writing new secrets to OpenBao KV ({})...", sorted_paths.join(", ")));

        let pf = secrets::port_forward("data", &ob_pod, 8200).await
            .map_err(|e| step_err(e.to_string()))?;
        let bao = BaoClient::with_token(
            &format!("http://127.0.0.1:{}", pf.local_port),
            &root_token,
        );

        let creds: HashMap<String, String> = data.get("creds")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let dirty_set: HashSet<&str> = dirty_paths.iter().map(|s| s.as_str()).collect();

        let kv_paths = [
            "hydra", "kratos", "seaweedfs", "gitea", "hive", "livekit",
            "people", "login-ui", "kratos-admin", "docs", "meet", "drive",
            "projects", "calendars", "messages", "collabora", "tuwunel",
            "grafana", "scaleway-s3", "penpot",
        ];

        for path in kv_paths {
            if dirty_set.contains(path) {
                let json_key = format!("kv_data/{path}");
                if let Some(json_str) = creds.get(&json_key) {
                    let path_data: HashMap<String, String> = serde_json::from_str(json_str)
                        .map_err(|e| step_err(e.to_string()))?;
                    bao.kv_patch("secret", path, &path_data).await
                        .map_err(|e| step_err(e.to_string()))?;
                }
            }
        }

        Ok(ExecutionResult::next())
    }
}

// ── ConfigureKubernetesAuth ─────────────────────────────────────────────────

/// Enable Kubernetes auth, configure it, write VSO policy and role.
///
/// Reads: `skip_seed`, `ob_pod`, `root_token`
#[derive(Default)]
pub struct ConfigureKubernetesAuth;

#[async_trait::async_trait]
impl StepBody for ConfigureKubernetesAuth {
    async fn run(
        &mut self,
        ctx: &StepExecutionContext<'_>,
    ) -> wfe_core::Result<ExecutionResult> {
        let data = &ctx.workflow.data;

        if json_bool(data, "skip_seed") {
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match json_str(data, "ob_pod") {
            Some(p) => p,
            None => return Ok(ExecutionResult::next()),
        };
        let root_token = match json_str(data, "root_token") {
            Some(t) => t,
            None => return Ok(ExecutionResult::next()),
        };

        ok("Configuring Kubernetes auth for VSO...");

        let pf = secrets::port_forward("data", &ob_pod, 8200).await
            .map_err(|e| step_err(e.to_string()))?;
        let bao = BaoClient::with_token(
            &format!("http://127.0.0.1:{}", pf.local_port),
            &root_token,
        );

        let _ = bao.auth_enable("kubernetes", "kubernetes").await;

        bao.write(
            "auth/kubernetes/config",
            &serde_json::json!({
                "kubernetes_host": "https://kubernetes.default.svc.cluster.local"
            }),
        ).await.map_err(|e| step_err(e.to_string()))?;

        let policy_hcl = concat!(
            "path \"secret/data/*\" { capabilities = [\"read\"] }\n",
            "path \"secret/metadata/*\" { capabilities = [\"read\", \"list\"] }\n",
            "path \"database/static-creds/*\" { capabilities = [\"read\"] }\n",
        );
        bao.write_policy("vso-reader", policy_hcl).await
            .map_err(|e| step_err(e.to_string()))?;

        bao.write(
            "auth/kubernetes/role/vso",
            &serde_json::json!({
                "bound_service_account_names": "default",
                "bound_service_account_namespaces": "ory,devtools,storage,lasuite,matrix,media,data,monitoring,cert-manager",
                "policies": "vso-reader",
                "ttl": "1h"
            }),
        ).await.map_err(|e| step_err(e.to_string()))?;

        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::WorkflowStatus;

    async fn run_step<S: StepBody + Default + 'static>(
        data: serde_json::Value,
    ) -> wfe_core::models::WorkflowInstance {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        host.register_step::<S>().await;
        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<S>()
            .name("test-step")
            .end_workflow()
            .build("test-wf", 1);
        host.register_workflow_definition(def).await;
        let instance = run_workflow_sync(&host, "test-wf", 1, data, Duration::from_secs(5))
            .await
            .unwrap();
        host.stop().await;
        instance
    }

    #[tokio::test]
    async fn test_seed_all_kv_paths_skip_seed() {
        let instance = run_step::<SeedAllKVPaths>(serde_json::json!({ "skip_seed": true })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_seed_all_kv_paths_no_ob_pod() {
        let instance = run_step::<SeedAllKVPaths>(serde_json::json!({ "skip_seed": false })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_seed_all_kv_paths_no_root_token() {
        let instance = run_step::<SeedAllKVPaths>(
            serde_json::json!({ "skip_seed": false, "ob_pod": "openbao-0" })
        ).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_write_dirty_kv_paths_skip_seed() {
        let instance = run_step::<WriteDirtyKVPaths>(serde_json::json!({ "skip_seed": true })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_write_dirty_kv_paths_empty_dirty_paths() {
        let instance = run_step::<WriteDirtyKVPaths>(serde_json::json!({
            "skip_seed": false, "ob_pod": "openbao-0", "root_token": "hvs.test", "dirty_paths": [],
        })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_configure_kubernetes_auth_skip_seed() {
        let instance = run_step::<ConfigureKubernetesAuth>(serde_json::json!({ "skip_seed": true })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_configure_kubernetes_auth_no_ob_pod() {
        let instance = run_step::<ConfigureKubernetesAuth>(serde_json::json!({ "skip_seed": false })).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }
}
