//! Service-oriented CLI commands: deploy, secrets, shell.
//!
//! These commands use the service registry for name resolution.

use crate::cli::SecretsAction;
use crate::error::{Result, SunbeamError};
use crate::output::{ok, step, warn};
use sunbeam_sdk::registry::{self, ServiceRegistry};

/// Discover the service registry from the cluster.
async fn get_registry() -> Result<ServiceRegistry> {
    let client = crate::kube::get_client().await?;
    registry::discover(client).await
        .map_err(|e| SunbeamError::Other(format!("service discovery failed: {e}")))
}

/// Deploy service(s) by name, category, or namespace.
///
/// Applies manifests for each unique namespace, then rollout-restarts the
/// specific deployments that belong to the resolved services.
pub async fn cmd_deploy(target: &str, domain: &str, email: &str) -> Result<()> {
    let reg = get_registry().await?;
    let resolved = reg.resolve(target);
    if resolved.is_empty() {
        bail!("Unknown service: '{target}'. Try 'sunbeam deploy --all' or a service name like 'hydra'.");
    }

    // Get unique namespaces
    let mut namespaces: Vec<&str> = resolved.iter().map(|s| s.namespace.as_str()).collect();
    namespaces.sort_unstable();
    namespaces.dedup();

    // Apply manifests for each namespace
    let is_production = !crate::config::active_context().ssh_host.is_empty();
    let env_str = if is_production { "production" } else { "local" };
    for ns in &namespaces {
        step(&format!("Applying manifests for {ns}..."));
        crate::manifests::cmd_apply(env_str, domain, email, ns).await?;
    }

    // Rollout restart the specific deployments
    for svc in &resolved {
        for deploy in &svc.deployments {
            step(&format!("Restarting {}/{}...", svc.namespace, deploy));
            crate::kube::kube_rollout_restart(&svc.namespace, deploy).await?;
        }
    }

    ok("Deploy complete.");
    Ok(())
}

/// View or get secrets for a service from OpenBao.
pub async fn cmd_secrets(service: &str, action: Option<SecretsAction>) -> Result<()> {
    let reg = get_registry().await?;
    let svc = reg.get(service)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{service}'")))?;

    let kv_path = svc
        .kv_path
        .as_deref()
        .ok_or_else(|| SunbeamError::Other(format!("Service '{service}' has no secrets in OpenBao")))?;

    // Port-forward to OpenBao and read the secret
    let ob_pod = crate::kube::find_pod_by_label(
        "data",
        "app.kubernetes.io/name=openbao,component=server",
    )
    .await
    .ok_or_else(|| SunbeamError::Other("OpenBao pod not found".into()))?;

    let pf = crate::secrets::port_forward("data", &ob_pod, 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);

    // Get root token from k8s secret
    let token = crate::kube::kube_get_secret_field("data", "openbao-keys", "root-token")
        .await
        .map_err(|_| SunbeamError::Other("Failed to get OpenBao root token".into()))?;

    let bao = crate::openbao::BaoClient::with_token(&bao_url, &token);

    match action {
        None => {
            // List all fields for the service
            match bao.kv_get("secret", kv_path).await? {
                Some(data) => {
                    step(&format!("Secrets for {service} (secret/{kv_path}):"));
                    let mut keys: Vec<&String> = data.keys().collect();
                    keys.sort();
                    for key in keys {
                        let value = &data[key];
                        // Mask values longer than 8 chars for security
                        let display = if value.len() > 8 {
                            format!("{}...{}", &value[..4], &value[value.len() - 4..])
                        } else {
                            value.clone()
                        };
                        println!("  {key}: {display}");
                    }
                }
                None => {
                    warn(&format!("No secrets found at secret/{kv_path}"));
                }
            }
        }
        Some(SecretsAction::Get { key }) => {
            let value = bao.kv_get_field("secret", kv_path, &key).await?;
            if value.is_empty() {
                warn(&format!("Field '{key}' not found in secret/{kv_path}"));
            } else {
                println!("{value}");
            }
        }
    }

    Ok(())
}

/// Interactive shell into a service pod.
///
/// Special-cases postgres for psql; everything else gets `/bin/sh`.
pub async fn cmd_shell(service: &str) -> Result<()> {
    let reg = get_registry().await?;
    let svc = reg.get(service)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{service}'")))?;

    let context = crate::kube::context();

    match service {
        "postgres" => {
            step("Connecting to PostgreSQL primary...");
            let pod = crate::kube::find_pod_by_label(
                "data",
                "cnpg.io/cluster=postgres,role=primary",
            )
            .await
            .ok_or_else(|| SunbeamError::Other("PostgreSQL primary pod not found".into()))?;

            let status = tokio::process::Command::new("kubectl")
                .args([
                    &format!("--context={context}"),
                    "exec",
                    "-it",
                    "-n",
                    "data",
                    &pod,
                    "--",
                    "psql",
                    "-U",
                    "postgres",
                ])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .await
                .map_err(|e| SunbeamError::Other(format!("Failed to exec into pod: {e}")))?;

            if !status.success() {
                warn("psql session ended with non-zero exit code");
            }
            Ok(())
        }
        _ => {
            if svc.deployments.is_empty() {
                bail!("Service '{service}' has no deployments");
            }
            let deploy = &svc.deployments[0];
            let pod = crate::kube::find_pod_by_label(
                &svc.namespace,
                &format!("app={deploy}"),
            )
            .await
            .ok_or_else(|| SunbeamError::Other(format!("No pod found for {service}")))?;

            step(&format!("Connecting to {service} ({pod})..."));
            let status = tokio::process::Command::new("kubectl")
                .args([
                    &format!("--context={context}"),
                    "exec",
                    "-it",
                    "-n",
                    &svc.namespace,
                    &pod,
                    "--",
                    "/bin/sh",
                ])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .await
                .map_err(|e| SunbeamError::Other(format!("Failed to exec into pod: {e}")))?;

            if !status.success() {
                warn("Shell session ended with non-zero exit code");
            }
            Ok(())
        }
    }
}
