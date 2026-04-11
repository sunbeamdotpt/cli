//! Steps for the bootstrap workflow — Gitea admin setup, org creation, OIDC.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::{ok, step, warn};
use crate::workflows::data::BootstrapData;

const GITEA_ADMIN_USER: &str = "gitea_admin";
const GITEA_ADMIN_EMAIL: &str = "gitea@local.domain";

fn load_data(ctx: &StepExecutionContext<'_>) -> wfe_core::Result<BootstrapData> {
    serde_json::from_value(ctx.workflow.data.clone())
        .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))
}

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

// ── GetAdminPassword ───────────────────────────────────────────────────────

/// Retrieve the Gitea admin password from the K8s secret.
#[derive(Default)]
pub struct GetAdminPassword;

#[async_trait::async_trait]
impl StepBody for GetAdminPassword {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = load_data(ctx)?;
        let step_ctx = data
            .ctx
            .as_ref()
            .ok_or_else(|| step_err("missing __ctx in workflow data"))?;

        k::set_context(&step_ctx.kube_context);

        let pass = k::kube_get_secret_field("devtools", "gitea-admin-credentials", "password")
            .await
            .unwrap_or_default();

        if pass.is_empty() {
            warn("gitea-admin-credentials password not found -- cannot bootstrap.");
            return Err(step_err("gitea-admin-credentials password not found"));
        }

        let domain = k::get_domain().await.map_err(|e| step_err(e.to_string()))?;

        let mut result = ExecutionResult::next();
        result.output_data = Some(serde_json::json!({
            "gitea_admin_pass": pass,
            "domain": domain,
        }));
        Ok(result)
    }
}

// ── WaitForGiteaPod ────────────────────────────────────────────────────────

/// Wait for a Running + Ready Gitea pod (up to 3 minutes).
#[derive(Default)]
pub struct WaitForGiteaPod;

#[async_trait::async_trait]
impl StepBody for WaitForGiteaPod {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        step("Waiting for Gitea pod...");

        let client = k::get_client().await.map_err(|e| step_err(e.to_string()))?;
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(client.clone(), "devtools");

        for _ in 0..60 {
            let lp = kube::api::ListParams::default().labels("app.kubernetes.io/name=gitea");
            if let Ok(pod_list) = pods.list(&lp).await {
                for pod in &pod_list.items {
                    let phase = pod
                        .status
                        .as_ref()
                        .and_then(|s| s.phase.as_deref())
                        .unwrap_or("");
                    if phase != "Running" {
                        continue;
                    }
                    let ready = pod
                        .status
                        .as_ref()
                        .and_then(|s| s.container_statuses.as_ref())
                        .and_then(|cs| cs.first())
                        .map(|c| c.ready)
                        .unwrap_or(false);
                    if ready {
                        let name = pod.metadata.name.as_deref().unwrap_or("").to_string();
                        if !name.is_empty() {
                            ok(&format!("Gitea pod ready: {name}"));
                            let mut result = ExecutionResult::next();
                            result.output_data = Some(serde_json::json!({ "gitea_pod": name }));
                            return Ok(result);
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }

        warn("Gitea pod not ready after 3 min -- skipping bootstrap.");
        Err(step_err("Gitea pod not ready after 3 minutes"))
    }
}

// ── SetAdminPassword ───────────────────────────────────────────────────────

/// Set the Gitea admin password via CLI.
#[derive(Default)]
pub struct SetAdminPassword;

#[async_trait::async_trait]
impl StepBody for SetAdminPassword {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = load_data(ctx)?;
        let pod = data
            .gitea_pod
            .as_deref()
            .ok_or_else(|| step_err("gitea_pod not set"))?;
        let password = data
            .gitea_admin_pass
            .as_deref()
            .ok_or_else(|| step_err("gitea_admin_pass not set"))?;

        let (code, output) = k::kube_exec(
            "devtools",
            pod,
            &[
                "gitea",
                "admin",
                "user",
                "change-password",
                "--username",
                GITEA_ADMIN_USER,
                "--password",
                password,
                "--must-change-password=false",
            ],
            Some("gitea"),
        )
        .await
        .map_err(|e| step_err(e.to_string()))?;

        if code == 0 || output.to_lowercase().contains("password") {
            ok(&format!("Admin '{GITEA_ADMIN_USER}' password set."));
        } else {
            warn(&format!("change-password: {output}"));
        }

        Ok(ExecutionResult::next())
    }
}

// ── MarkAdminPrivate ───────────────────────────────────────────────────────

/// Mark the admin account as private via API.
#[derive(Default)]
pub struct MarkAdminPrivate;

#[async_trait::async_trait]
impl StepBody for MarkAdminPrivate {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = load_data(ctx)?;
        let pod = data
            .gitea_pod
            .as_deref()
            .ok_or_else(|| step_err("gitea_pod not set"))?;
        let password = data
            .gitea_admin_pass
            .as_deref()
            .ok_or_else(|| step_err("gitea_admin_pass not set"))?;

        let body = serde_json::json!({
            "source_id": 0,
            "login_name": GITEA_ADMIN_USER,
            "email": GITEA_ADMIN_EMAIL,
            "visibility": "private",
        });

        let result = gitea_api(
            pod,
            "PATCH",
            &format!("/admin/users/{GITEA_ADMIN_USER}"),
            password,
            Some(&body),
        )
        .await?;

        if result.get("login").and_then(|v| v.as_str()) == Some(GITEA_ADMIN_USER) {
            ok(&format!("Admin '{GITEA_ADMIN_USER}' marked as private."));
        } else {
            warn(&format!("Could not set admin visibility: {result}"));
        }

        Ok(ExecutionResult::next())
    }
}

// ── CreateOrgs ─────────────────────────────────────────────────────────────

/// Create the studio and internal organizations.
#[derive(Default)]
pub struct CreateOrgs;

#[async_trait::async_trait]
impl StepBody for CreateOrgs {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = load_data(ctx)?;
        let pod = data
            .gitea_pod
            .as_deref()
            .ok_or_else(|| step_err("gitea_pod not set"))?;
        let password = data
            .gitea_admin_pass
            .as_deref()
            .ok_or_else(|| step_err("gitea_admin_pass not set"))?;

        let orgs = [
            ("studio", "public", "Public source code"),
            ("internal", "private", "Internal tools and services"),
        ];

        for (org_name, visibility, desc) in &orgs {
            let body = serde_json::json!({
                "username": org_name,
                "visibility": visibility,
                "description": desc,
            });

            let result = gitea_api(pod, "POST", "/orgs", password, Some(&body)).await?;

            if result.get("id").is_some() {
                ok(&format!("Created org '{org_name}'."));
            } else if result
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .contains("already")
            {
                ok(&format!("Org '{org_name}' already exists."));
            } else {
                let msg = result
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{result}"));
                warn(&format!("Org '{org_name}': {msg}"));
            }
        }

        Ok(ExecutionResult::next())
    }
}

// ── ConfigureOIDC ──────────────────────────────────────────────────────────

/// Configure Hydra as the OIDC authentication source.
#[derive(Default)]
pub struct ConfigureOIDC;

#[async_trait::async_trait]
impl StepBody for ConfigureOIDC {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = load_data(ctx)?;
        let pod = data
            .gitea_pod
            .as_deref()
            .ok_or_else(|| step_err("gitea_pod not set"))?;

        let (_, auth_list_output) = k::kube_exec(
            "devtools",
            pod,
            &["gitea", "admin", "auth", "list"],
            Some("gitea"),
        )
        .await
        .map_err(|e| step_err(e.to_string()))?;

        let mut existing_id: Option<String> = None;
        let mut exact_ok = false;

        for line in auth_list_output.lines().skip(1) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let src_id = parts[0].trim();
            let src_name = parts[1].trim();

            if src_name == "Sunbeam" {
                exact_ok = true;
                break;
            }

            let src_type = if parts.len() > 2 { parts[2].trim() } else { "" };
            if src_name == "Sunbeam Auth"
                || (src_name.starts_with("Sunbeam") && src_type == "OAuth2")
            {
                existing_id = Some(src_id.to_string());
            }
        }

        if exact_ok {
            ok("OIDC auth source 'Sunbeam' already present.");
            return Ok(ExecutionResult::next());
        }

        if let Some(eid) = existing_id {
            let (code, stderr) = k::kube_exec(
                "devtools",
                pod,
                &[
                    "gitea",
                    "admin",
                    "auth",
                    "update-oauth",
                    "--id",
                    &eid,
                    "--name",
                    "Sunbeam",
                ],
                Some("gitea"),
            )
            .await
            .map_err(|e| step_err(e.to_string()))?;

            if code == 0 {
                ok(&format!(
                    "Renamed OIDC auth source (id={eid}) to 'Sunbeam'."
                ));
            } else {
                warn(&format!("Rename failed: {stderr}"));
            }
            return Ok(ExecutionResult::next());
        }

        // Create new OIDC auth source
        let oidc_id = k::kube_get_secret_field("devtools", "oidc-gitea", "CLIENT_ID").await;
        let oidc_secret = k::kube_get_secret_field("devtools", "oidc-gitea", "CLIENT_SECRET").await;

        match (oidc_id, oidc_secret) {
            (Ok(oidc_id), Ok(oidc_sec)) => {
                let discover_url = "http://hydra-public.ory.svc.cluster.local:4444/.well-known/openid-configuration";

                let (code, stderr) = k::kube_exec(
                    "devtools",
                    pod,
                    &[
                        "gitea",
                        "admin",
                        "auth",
                        "add-oauth",
                        "--name",
                        "Sunbeam",
                        "--provider",
                        "openidConnect",
                        "--key",
                        &oidc_id,
                        "--secret",
                        &oidc_sec,
                        "--auto-discover-url",
                        discover_url,
                        "--scopes",
                        "openid",
                        "--scopes",
                        "email",
                        "--scopes",
                        "profile",
                    ],
                    Some("gitea"),
                )
                .await
                .map_err(|e| step_err(e.to_string()))?;

                if code == 0 {
                    ok("OIDC auth source 'Sunbeam' configured.");
                } else {
                    warn(&format!("OIDC auth source config failed: {stderr}"));
                }
            }
            _ => {
                warn("oidc-gitea secret not found -- OIDC auth source not configured.");
            }
        }

        Ok(ExecutionResult::next())
    }
}

// ── PrintBootstrapResult ───────────────────────────────────────────────────

/// Print the final bootstrap result.
#[derive(Default)]
pub struct PrintBootstrapResult;

#[async_trait::async_trait]
impl StepBody for PrintBootstrapResult {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data = load_data(ctx)?;
        let domain = data.domain.as_deref().unwrap_or("unknown");
        ok(&format!(
            "Gitea ready -- https://src.{domain} ({GITEA_ADMIN_USER} / <from openbao>)"
        ));
        Ok(ExecutionResult::next())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Call Gitea API via kubectl curl inside the pod.
async fn gitea_api(
    pod: &str,
    method: &str,
    path: &str,
    password: &str,
    data: Option<&serde_json::Value>,
) -> wfe_core::Result<serde_json::Value> {
    let url = format!("http://localhost:3000/api/v1{path}");
    let auth = format!("{GITEA_ADMIN_USER}:{password}");

    let mut args = vec![
        "curl",
        "-s",
        "-X",
        method,
        &url,
        "-H",
        "Content-Type: application/json",
        "-u",
        &auth,
    ];

    let data_str;
    if let Some(d) = data {
        data_str = serde_json::to_string(d).map_err(|e| step_err(e.to_string()))?;
        args.push("-d");
        args.push(&data_str);
    }

    let (_, stdout) = k::kube_exec("devtools", pod, &args, Some("gitea"))
        .await
        .map_err(|e| step_err(e.to_string()))?;

    Ok(serde_json::from_str(&stdout).unwrap_or(serde_json::Value::Object(Default::default())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_admin_password_is_default() {
        let _ = GetAdminPassword;
    }

    #[test]
    fn wait_for_gitea_pod_is_default() {
        let _ = WaitForGiteaPod;
    }

    #[test]
    fn set_admin_password_is_default() {
        let _ = SetAdminPassword;
    }

    #[test]
    fn mark_admin_private_is_default() {
        let _ = MarkAdminPrivate;
    }

    #[test]
    fn create_orgs_is_default() {
        let _ = CreateOrgs;
    }

    #[test]
    fn configure_oidc_is_default() {
        let _ = ConfigureOIDC;
    }

    #[test]
    fn print_bootstrap_result_is_default() {
        let _ = PrintBootstrapResult;
    }

    #[test]
    fn test_constants() {
        assert_eq!(GITEA_ADMIN_USER, "gitea_admin");
        assert_eq!(GITEA_ADMIN_EMAIL, "gitea@local.domain");
    }
}
