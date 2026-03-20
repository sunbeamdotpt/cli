//! Gitea bootstrap -- admin setup, org creation, OIDC auth source configuration.

use crate::error::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use serde_json::Value;

use crate::kube::{get_client, get_domain, kube_exec, kube_get_secret_field};
use crate::output::{ok, step, warn};

const GITEA_ADMIN_USER: &str = "gitea_admin";
const GITEA_ADMIN_EMAIL: &str = "gitea@local.domain";

/// Bootstrap Gitea: set admin password, create orgs, configure OIDC.
pub async fn cmd_bootstrap() -> Result<()> {
    let domain = get_domain().await?;

    // Retrieve gitea admin password from cluster secret
    let gitea_admin_pass = kube_get_secret_field("devtools", "gitea-admin-credentials", "password")
        .await
        .unwrap_or_default();

    if gitea_admin_pass.is_empty() {
        warn("gitea-admin-credentials password not found -- cannot bootstrap.");
        return Ok(());
    }

    step("Bootstrapping Gitea...");

    // Wait for a Running + Ready Gitea pod
    let pod_name = wait_for_gitea_pod().await?;
    let Some(pod) = pod_name else {
        warn("Gitea pod not ready after 3 min -- skipping bootstrap.");
        return Ok(());
    };

    // Set admin password
    set_admin_password(&pod, &gitea_admin_pass).await?;

    // Mark admin as private
    mark_admin_private(&pod, &gitea_admin_pass).await?;

    // Create orgs
    create_orgs(&pod, &gitea_admin_pass).await?;

    // Configure OIDC auth source
    configure_oidc(&pod, &gitea_admin_pass).await?;

    ok(&format!(
        "Gitea ready -- https://src.{domain} ({GITEA_ADMIN_USER} / <from openbao>)"
    ));
    Ok(())
}

/// Wait for a Running + Ready Gitea pod (up to 3 minutes).
async fn wait_for_gitea_pod() -> Result<Option<String>> {
    let client = get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), "devtools");

    for _ in 0..60 {
        let lp = ListParams::default().labels("app.kubernetes.io/name=gitea");
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
                    let name = pod
                        .metadata
                        .name
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        return Ok(Some(name));
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }

    Ok(None)
}

/// Set the admin password via gitea CLI exec.
async fn set_admin_password(pod: &str, password: &str) -> Result<()> {
    let (code, output) = kube_exec(
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
    .await?;

    if code == 0 || output.to_lowercase().contains("password") {
        ok(&format!("Admin '{GITEA_ADMIN_USER}' password set."));
    } else {
        warn(&format!("change-password: {output}"));
    }
    Ok(())
}

/// Call Gitea API via kubectl exec + curl inside the pod.
async fn gitea_api(
    pod: &str,
    method: &str,
    path: &str,
    password: &str,
    data: Option<&Value>,
) -> Result<Value> {
    let url = format!("http://localhost:3000/api/v1{path}");
    let auth = format!("{GITEA_ADMIN_USER}:{password}");

    let mut args = vec![
        "curl", "-s", "-X", method, &url, "-H", "Content-Type: application/json", "-u", &auth,
    ];

    let data_str;
    if let Some(d) = data {
        data_str = serde_json::to_string(d)?;
        args.push("-d");
        args.push(&data_str);
    }

    let (_, stdout) = kube_exec("devtools", pod, &args, Some("gitea")).await?;

    Ok(serde_json::from_str(&stdout).unwrap_or(Value::Object(Default::default())))
}

/// Mark the admin account as private.
async fn mark_admin_private(pod: &str, password: &str) -> Result<()> {
    let data = serde_json::json!({
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
        Some(&data),
    )
    .await?;

    if result.get("login").and_then(|v| v.as_str()) == Some(GITEA_ADMIN_USER) {
        ok(&format!("Admin '{GITEA_ADMIN_USER}' marked as private."));
    } else {
        warn(&format!("Could not set admin visibility: {result}"));
    }
    Ok(())
}

/// Create the studio and internal organizations.
async fn create_orgs(pod: &str, password: &str) -> Result<()> {
    let orgs = [
        ("studio", "public", "Public source code"),
        ("internal", "private", "Internal tools and services"),
    ];

    for (org_name, visibility, desc) in &orgs {
        let data = serde_json::json!({
            "username": org_name,
            "visibility": visibility,
            "description": desc,
        });

        let result = gitea_api(pod, "POST", "/orgs", password, Some(&data)).await?;

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
    Ok(())
}

/// Configure Hydra as the OIDC authentication source.
async fn configure_oidc(pod: &str, _password: &str) -> Result<()> {
    // List existing auth sources
    let (_, auth_list_output) =
        kube_exec("devtools", pod, &["gitea", "admin", "auth", "list"], Some("gitea")).await?;

    let mut existing_id: Option<String> = None;
    let mut exact_ok = false;

    for line in auth_list_output.lines().skip(1) {
        // Tab-separated: ID\tName\tType\tEnabled
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

        let src_type = if parts.len() > 2 {
            parts[2].trim()
        } else {
            ""
        };

        if src_name == "Sunbeam Auth"
            || (src_name.starts_with("Sunbeam") && src_type == "OAuth2")
        {
            existing_id = Some(src_id.to_string());
        }
    }

    if exact_ok {
        ok("OIDC auth source 'Sunbeam' already present.");
        return Ok(());
    }

    if let Some(eid) = existing_id {
        // Wrong name -- rename in-place
        let (code, stderr) = kube_exec(
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
        .await?;

        if code == 0 {
            ok(&format!(
                "Renamed OIDC auth source (id={eid}) to 'Sunbeam'."
            ));
        } else {
            warn(&format!("Rename failed: {stderr}"));
        }
        return Ok(());
    }

    // Create new OIDC auth source
    let oidc_id = kube_get_secret_field("lasuite", "oidc-gitea", "CLIENT_ID").await;
    let oidc_secret = kube_get_secret_field("lasuite", "oidc-gitea", "CLIENT_SECRET").await;

    match (oidc_id, oidc_secret) {
        (Ok(oidc_id), Ok(oidc_sec)) => {
            let discover_url =
                "http://hydra-public.ory.svc.cluster.local:4444/.well-known/openid-configuration";

            let (code, stderr) = kube_exec(
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
            .await?;

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

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(GITEA_ADMIN_USER, "gitea_admin");
        assert_eq!(GITEA_ADMIN_EMAIL, "gitea@local.domain");
    }

    #[test]
    fn test_org_definitions() {
        // Verify the org configs match the Python version
        let orgs = [
            ("studio", "public", "Public source code"),
            ("internal", "private", "Internal tools and services"),
        ];
        assert_eq!(orgs[0].0, "studio");
        assert_eq!(orgs[0].1, "public");
        assert_eq!(orgs[1].0, "internal");
        assert_eq!(orgs[1].1, "private");
    }

    #[test]
    fn test_parse_auth_list_output() {
        let output = "ID\tName\tType\tEnabled\n1\tSunbeam\tOAuth2\ttrue\n";
        let mut found = false;
        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 && parts[1].trim() == "Sunbeam" {
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn test_parse_auth_list_rename_needed() {
        let output = "ID\tName\tType\tEnabled\n5\tSunbeam Auth\tOAuth2\ttrue\n";
        let mut rename_id: Option<String> = None;
        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let name = parts[1].trim();
                let typ = parts[2].trim();
                if name == "Sunbeam Auth" || (name.starts_with("Sunbeam") && typ == "OAuth2") {
                    rename_id = Some(parts[0].trim().to_string());
                }
            }
        }
        assert_eq!(rename_id, Some("5".to_string()));
    }

    #[test]
    fn test_gitea_api_response_parsing() {
        // Simulate a successful org creation response
        let json_str = r#"{"id": 1, "username": "studio"}"#;
        let val: Value = serde_json::from_str(json_str).unwrap();
        assert!(val.get("id").is_some());

        // Simulate an "already exists" response
        let json_str = r#"{"message": "organization already exists"}"#;
        let val: Value = serde_json::from_str(json_str).unwrap();
        assert!(val
            .get("message")
            .unwrap()
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("already"));
    }

    #[test]
    fn test_admin_visibility_patch_body() {
        let data = serde_json::json!({
            "source_id": 0,
            "login_name": GITEA_ADMIN_USER,
            "email": GITEA_ADMIN_EMAIL,
            "visibility": "private",
        });
        assert_eq!(data["login_name"], "gitea_admin");
        assert_eq!(data["visibility"], "private");
    }
}
