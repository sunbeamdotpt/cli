//! Service-oriented CLI commands: deploy, secrets, shell, describe, exec, port-forward, scale, top, edit.
//!
//! These commands use the service registry for name resolution and delegate to
//! kubectl for interactive operations.

use crate::cli::{SecretsAction, ServiceAction};
use crate::error::{Result, SunbeamError};
use crate::output::{ok, step, warn};
use sunbeam_sdk::registry::{self, ServiceRegistry};

/// Discover the service registry from the cluster.
async fn get_registry() -> Result<ServiceRegistry> {
    let client = crate::kube::get_client().await?;
    registry::discover(client)
        .await
        .map_err(|e| SunbeamError::Other(format!("service discovery failed: {e}")))
}

/// Resolve a service by name, returning (namespace, first deployment name).
async fn resolve_service(name: &str) -> Result<(String, String)> {
    let reg = get_registry().await?;
    let svc = reg
        .get(name)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{name}'")))?;
    if svc.deployments.is_empty() {
        bail!("Service '{name}' has no deployments");
    }
    Ok((svc.namespace.clone(), svc.deployments[0].clone()))
}

/// Top-level dispatcher for `sunbeam service <action>`.
pub async fn dispatch(action: ServiceAction, domain: &str, email: &str) -> Result<()> {
    match action {
        ServiceAction::Status { target } => {
            crate::services::cmd_status(target.as_deref()).await
        }
        ServiceAction::Logs { target, follow } => {
            crate::services::cmd_logs(&target, follow).await
        }
        ServiceAction::Get { target, output } => {
            crate::services::cmd_get(&target, &output).await
        }
        ServiceAction::Restart { target } => {
            crate::services::cmd_restart(target.as_deref()).await
        }
        ServiceAction::Check { target } => {
            crate::checks::cmd_check(target.as_deref()).await
        }
        ServiceAction::Deploy { target, all } => {
            if all || target.is_none() {
                let is_production = !crate::config::active_context().ssh_host.is_empty();
                let env_str = if is_production { "production" } else { "local" };
                crate::manifests::cmd_apply(env_str, domain, email, "").await
            } else {
                cmd_deploy(&target.unwrap(), domain, email).await
            }
        }
        ServiceAction::Apply { namespace, apply_all, domain: apply_domain, email: apply_email } => {
            let is_production = !crate::config::active_context().ssh_host.is_empty();
            let env_str = if is_production { "production" } else { "local" };
            let d = if apply_domain.is_empty() { domain.to_string() } else { apply_domain };
            let e = if apply_email.is_empty() { email.to_string() } else { apply_email };
            let ns = namespace.unwrap_or_default();

            if is_production && ns.is_empty() && !apply_all {
                crate::output::warn("This will apply ALL namespaces to production.");
                eprint!("  Continue? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            crate::manifests::cmd_apply(env_str, &d, &e, &ns).await
        }
        ServiceAction::Seed => {
            crate::output::step("Seeding secrets (workflow engine)...");
            run_workflow("seed", 2, 900, |i| crate::workflows::seed::print_summary(i)).await
        }
        ServiceAction::Verify => {
            crate::output::step("Verifying VSO -> OpenBao integration...");
            run_workflow("verify", 1, 300, |i| crate::workflows::verify::print_summary(i)).await
        }
        ServiceAction::Secrets { service, action } => cmd_secrets(&service, action).await,
        ServiceAction::Shell { service } => cmd_shell(&service).await,
        ServiceAction::Describe { service } => cmd_describe(&service).await,
        ServiceAction::Exec {
            service,
            container,
            command,
        } => cmd_exec(&service, container.as_deref(), &command).await,
        ServiceAction::PortForward { service, ports } => {
            cmd_port_forward(&service, &ports).await
        }
        ServiceAction::Scale { service, replicas } => cmd_scale(&service, replicas).await,
        ServiceAction::Top { service } => cmd_top(&service).await,
        ServiceAction::Edit { service } => cmd_edit(&service).await,
    }
}

/// Helper: run a named workflow via the in-process engine.
async fn run_workflow(
    name: &str,
    version: u32,
    timeout_secs: u64,
    print_summary: impl FnOnce(&wfe_core::models::WorkflowInstance),
) -> Result<()> {
    let ctx_name = {
        let cfg = crate::config::load_config();
        if cfg.current_context.is_empty() {
            "default".to_string()
        } else {
            cfg.current_context.clone()
        }
    };

    let host = crate::workflows::host::create_host(&ctx_name).await?;

    // Register the workflow definition
    match name {
        "seed" => crate::workflows::seed::register(&host).await,
        "verify" => crate::workflows::verify::register(&host).await,
        _ => {}
    }

    let step_ctx = crate::workflows::StepContext::from_active();
    let initial_data = serde_json::json!({ "__ctx": step_ctx });

    let instance = wfe::run_workflow_sync(
        &host,
        name,
        version,
        initial_data,
        std::time::Duration::from_secs(timeout_secs),
    )
    .await
    .map_err(|e| SunbeamError::Other(format!("{name} workflow failed: {e}")))?;

    print_summary(&instance);
    crate::workflows::host::shutdown_host(host).await;

    if instance.status != wfe_core::models::WorkflowStatus::Complete {
        return Err(SunbeamError::Other(format!(
            "{name} workflow ended with status {:?}",
            instance.status
        )));
    }

    Ok(())
}

/// Deploy service(s) by name, category, or namespace.
async fn cmd_deploy(target: &str, domain: &str, email: &str) -> Result<()> {
    let reg = get_registry().await?;
    let resolved = reg.resolve(target);
    if resolved.is_empty() {
        bail!("Unknown service: '{target}'. Try 'sunbeam service deploy --all' or a service name like 'hydra'.");
    }

    let mut namespaces: Vec<&str> = resolved.iter().map(|s| s.namespace.as_str()).collect();
    namespaces.sort_unstable();
    namespaces.dedup();

    let is_production = !crate::config::active_context().ssh_host.is_empty();
    let env_str = if is_production { "production" } else { "local" };
    for ns in &namespaces {
        step(&format!("Applying manifests for {ns}..."));
        crate::manifests::cmd_apply(env_str, domain, email, ns).await?;
    }

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
async fn cmd_secrets(service: &str, action: Option<SecretsAction>) -> Result<()> {
    let reg = get_registry().await?;
    let svc = reg
        .get(service)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{service}'")))?;

    let kv_path = svc
        .kv_path
        .as_deref()
        .ok_or_else(|| SunbeamError::Other(format!("Service '{service}' has no secrets in OpenBao")))?;

    let ob_pod = crate::kube::find_pod_by_label(
        "data",
        "app.kubernetes.io/name=openbao,component=server",
    )
    .await
    .ok_or_else(|| SunbeamError::Other("OpenBao pod not found".into()))?;

    let pf = crate::secrets::port_forward("data", &ob_pod, 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);

    let token = crate::kube::kube_get_secret_field("data", "openbao-keys", "root-token")
        .await
        .map_err(|_| SunbeamError::Other("Failed to get OpenBao root token".into()))?;

    let bao = crate::openbao::BaoClient::with_token(&bao_url, &token);

    match action {
        None => {
            match bao.kv_get("secret", kv_path).await? {
                Some(data) => {
                    step(&format!("Secrets for {service} (secret/{kv_path}):"));
                    let mut keys: Vec<&String> = data.keys().collect();
                    keys.sort();
                    for key in keys {
                        let value = &data[key];
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
async fn cmd_shell(service: &str) -> Result<()> {
    let reg = get_registry().await?;
    let svc = reg
        .get(service)
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

            kubectl_interactive(&[
                &format!("--context={context}"),
                "exec", "-it", "-n", "data", &pod, "--", "psql", "-U", "postgres",
            ])
            .await
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
            kubectl_interactive(&[
                &format!("--context={context}"),
                "exec", "-it", "-n", &svc.namespace, &pod, "--", "/bin/sh",
            ])
            .await
        }
    }
}

/// Describe a service's deployment (kubectl describe).
async fn cmd_describe(service: &str) -> Result<()> {
    let (ns, deploy) = resolve_service(service).await?;
    let context = crate::kube::context();
    kubectl_interactive(&[
        &format!("--context={context}"),
        "describe", "deployment", &deploy, "-n", &ns,
    ])
    .await
}

/// Exec into a service pod with an optional command.
async fn cmd_exec(service: &str, container: Option<&str>, command: &[String]) -> Result<()> {
    let (ns, deploy) = resolve_service(service).await?;
    let context = crate::kube::context();

    let pod = crate::kube::find_pod_by_label(&ns, &format!("app={deploy}"))
        .await
        .ok_or_else(|| SunbeamError::Other(format!("No pod found for {service}")))?;

    let mut args = vec![
        format!("--context={context}"),
        "exec".into(),
        "-it".into(),
        "-n".into(),
        ns,
        pod,
    ];
    if let Some(c) = container {
        args.push("-c".into());
        args.push(c.to_string());
    }
    args.push("--".into());
    if command.is_empty() {
        args.push("/bin/sh".into());
    } else {
        args.extend(command.iter().cloned());
    }

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    kubectl_interactive(&args_ref).await
}

/// Port-forward to a service pod.
async fn cmd_port_forward(service: &str, ports: &[String]) -> Result<()> {
    if ports.is_empty() {
        bail!("At least one port mapping required (e.g. '8080:80' or '8080')");
    }
    let (ns, deploy) = resolve_service(service).await?;
    let context = crate::kube::context();

    let pod = crate::kube::find_pod_by_label(&ns, &format!("app={deploy}"))
        .await
        .ok_or_else(|| SunbeamError::Other(format!("No pod found for {service}")))?;

    let mut args = vec![
        format!("--context={context}"),
        "port-forward".into(),
        "-n".into(),
        ns,
        pod,
    ];
    args.extend(ports.iter().cloned());

    step(&format!("Port-forwarding to {service}..."));
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    kubectl_interactive(&args_ref).await
}

/// Scale a service deployment.
async fn cmd_scale(service: &str, replicas: u32) -> Result<()> {
    let (ns, deploy) = resolve_service(service).await?;
    let context = crate::kube::context();

    step(&format!("Scaling {service} to {replicas} replica(s)..."));
    let status = tokio::process::Command::new("kubectl")
        .args([
            &format!("--context={context}"),
            "scale",
            "deployment",
            &deploy,
            "-n",
            &ns,
            &format!("--replicas={replicas}"),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| SunbeamError::Other(format!("kubectl scale failed: {e}")))?;

    if status.success() {
        ok(&format!("{service} scaled to {replicas}."));
    } else {
        warn(&format!("kubectl scale exited with code {}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

/// Show resource usage for a service's pods.
async fn cmd_top(service: &str) -> Result<()> {
    let (ns, deploy) = resolve_service(service).await?;
    let context = crate::kube::context();
    kubectl_interactive(&[
        &format!("--context={context}"),
        "top", "pod", "-n", &ns, "-l", &format!("app={deploy}"),
    ])
    .await
}

/// Edit a service's deployment in-cluster.
async fn cmd_edit(service: &str) -> Result<()> {
    let (ns, deploy) = resolve_service(service).await?;
    let context = crate::kube::context();
    kubectl_interactive(&[
        &format!("--context={context}"),
        "edit", "deployment", &deploy, "-n", &ns,
    ])
    .await
}

/// Run kubectl with inherited stdio (interactive).
async fn kubectl_interactive(args: &[&str]) -> Result<()> {
    let status = tokio::process::Command::new("kubectl")
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .map_err(|e| SunbeamError::Other(format!("kubectl failed: {e}")))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        if code != 0 {
            warn(&format!("kubectl exited with code {code}"));
        }
    }
    Ok(())
}
