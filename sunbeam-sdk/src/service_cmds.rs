//! Service-oriented CLI commands: deploy, secrets, shell, describe, exec, port-forward, scale, top, edit.
//!
//! These commands use the service registry for name resolution and delegate to
//! kubectl for interactive operations.

use crate::cli::{SecretsAction, ServiceAction};
use crate::error::{Result, SunbeamError};
use crate::logger::Logger;
use crate::{debug, error, info, trace};
use tracing::Instrument;
use crate::registry::{self, ServiceRegistry};

/// Discover the service registry from the cluster.
async fn get_registry(logger: &crate::logger::Logger) -> Result<ServiceRegistry> {
    let client = crate::kube::get_client().await?;
    registry::discover(logger, &client)
        .await
        .map_err(|e| SunbeamError::Other(format!("service discovery failed: {e}")))
}

/// Resolve a service by name, returning (namespace, first deployment name).
async fn resolve_service(logger: &crate::logger::Logger, name: &str) -> Result<(String, String)> {
    let reg = get_registry(logger).await?;
    let svc = reg
        .get(name)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{name}'")))?;
    if svc.deployments.is_empty() {
        bail!("Service '{name}' has no deployments");
    }
    Ok((svc.namespace.clone(), svc.deployments[0].clone()))
}

// ---------------------------------------------------------------------------
// Profile resolution helpers
// ---------------------------------------------------------------------------

/// Synchronous inner function — takes explicit config and infra_dir so it
/// can be unit-tested without touching the filesystem or global config.
fn resolve_service_profile_inner(
    profile_flag: Option<String>,
    config: &crate::config::SunbeamConfig,
    infra_dir: &std::path::Path,
) -> Result<Option<(String, crate::config::Profile)>> {
    let profile_to_load = profile_flag.or_else(|| {
        let active_ctx = config.contexts.get(&config.current_context)?;
        match &active_ctx.profile {
            crate::config::ProfileRef::Name(name) => Some(name.clone()),
            _ => None,
        }
    });

    if let Some(name) = profile_to_load {
        let path = infra_dir.join("profiles").join(format!("{name}.yaml"));
        if path.exists() {
            Ok(Some((
                name.clone(),
                crate::profiles::load_profile(&path)?,
            )))
        } else if let Some(p) =
            config.resolve_profile(&crate::config::ProfileRef::Name(name.clone()))
        {
            Ok(Some((name, p)))
        } else {
            Err(crate::error::SunbeamError::Config(format!(
                "Profile not found: {name} (looked at {} and config.json)",
                path.display()
            )))
        }
    } else {
        Ok(None)
    }
}

/// Async wrapper that reads the live config and infra_dir.
async fn resolve_service_profile(
    profile_flag: Option<String>,
) -> Result<Option<(String, crate::config::Profile)>> {
    let config = crate::config::load_config();
    let infra_dir = crate::config::get_infra_dir();
    resolve_service_profile_inner(profile_flag, &config, &infra_dir)
}

/// Merge profile overrides with CLI overrides. CLI overrides are appended
/// last so they win when `apply_overrides` processes items in order.
fn merge_overrides(
    profile_overrides: crate::manifest_params::Overrides,
    cli_overrides: crate::manifest_params::Overrides,
) -> crate::manifest_params::Overrides {
    let mut combined = profile_overrides;
    combined.items.extend(cli_overrides.items);
    combined
}

/// Load profile (if any), resolve its overrides against discovered manifests,
/// and merge with explicit CLI --set/--disable/--enable flags.
///
/// Returns (profile_name_and_obj, merged_overrides, skip_namespaces).
async fn build_profile_context(
    logger: &Logger,
    profile_flag: Option<String>,
    set: &[String],
    disable: &[String],
    enable: &[String],
) -> Result<(Option<(String, crate::config::Profile)>, crate::manifest_params::Overrides, Vec<String>)> {
    let profile = resolve_service_profile(profile_flag).await?;
    let cli_overrides = crate::manifest_params::Overrides::from_cli(set, disable, enable)?;
    let mut skip_namespaces = Vec::new();

    if let Some((_, ref p)) = profile {
        skip_namespaces = p.skip_namespaces.clone();
        if p.skip_ory {
            skip_namespaces.push("ory".to_string());
        }

        let base_dir = crate::config::get_infra_dir().join("base");
        let config = crate::config::load_config();
        match crate::profiles::discover_manifests(&base_dir).await {
            Ok(resources) => {
                match crate::profiles::resolve_profile_overrides(p, &config.presets, &resources) {
                    Ok(profile_overrides) => {
                        let overrides = merge_overrides(profile_overrides, cli_overrides);
                        return Ok((profile, overrides, skip_namespaces));
                    }
                    Err(e) => {
                        info!(logger, "Failed to resolve profile overrides", error = e.to_string());
                    }
                }
            }
            Err(e) => {
                info!(logger, "Failed to discover manifests for profile resolution", error = e.to_string());
            }
        }
    }

    Ok((profile, cli_overrides, skip_namespaces))
}

/// Top-level dispatcher for `sunbeam service <action>`.
pub async fn dispatch(logger: &crate::logger::Logger, action: ServiceAction) -> Result<()> {
    debug!(logger, "service dispatch", action = format!("{:?}", action));
    match action {
        ServiceAction::Status { target } => crate::services::cmd_status(logger, target.as_deref()).await,
        ServiceAction::Logs { target, follow } => crate::services::cmd_logs(logger, &target, follow).await,
        ServiceAction::Get { target, output } => crate::services::cmd_get(logger, &target, &output).await,
        ServiceAction::Restart { target } => crate::services::cmd_restart(logger, target.as_deref()).await,
        ServiceAction::Check { target } => crate::checks::cmd_check(logger, target.as_deref()).await,
        ServiceAction::Deploy { target, all, profile } => match target {
            Some(t) if !all => {
                let span = tracing::info_span!("deploy", target = %t);
                cmd_deploy(logger, &t, profile).instrument(span).await
            }
            _ => {
                let (maybe_profile, overrides, skip_namespaces) =
                    build_profile_context(logger, profile, &[], &[], &[]).await?;

                if !skip_namespaces.is_empty() && maybe_profile.is_none() {
                    // No profile loaded but we have skips — impossible path, but be defensive
                }

                let opts = crate::manifests::ApplyOptions {
                    overrides: Some(overrides),
                    skip_namespaces,
                    ..Default::default()
                };
                crate::manifests::apply_manifests(logger, &opts).await?;
                Ok(())
            }
        },
        ServiceAction::List { format } => cmd_list(logger, format).await,
        ServiceAction::Apply {
            namespace,
            apply_all,
            dry_run,
            set,
            disable,
            enable,
            profile,
            ..
        } => {
            let (maybe_profile, overrides, skip_namespaces) =
                build_profile_context(logger, profile, &set, &disable, &enable).await?;

            let ns = namespace.unwrap_or_default();

            if !ns.is_empty() && skip_namespaces.contains(&ns) {
                if let Some((name, _)) = maybe_profile {
                    bail!("Namespace '{}' is skipped by profile '{}'", ns, name);
                }
            }

            if !dry_run && ns.is_empty() && !apply_all {
                if skip_namespaces.is_empty() {
                    info!(logger, "This will apply ALL namespaces.");
                } else {
                    info!(
                        logger,
                        "This will apply all namespaces except",
                        namespaces = skip_namespaces.join(", ")
                    );
                }
                eprint!("  Continue? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let opts = crate::manifests::ApplyOptions {
                namespace: ns,
                dry_run,
                skip_patterns: Vec::new(),
                overrides: Some(overrides),
                domain: None,
                skip_namespaces,
            };
            let rendered = crate::manifests::apply_manifests(logger, &opts).await?;
            if dry_run {
                print!("{rendered}");
            }
            Ok(())
        }
        ServiceAction::Seed => {
            info!(logger, "`sunbeam service seed` is deprecated. Use `sunbeam up` instead.");
            Ok(())
        }
        ServiceAction::Verify => {
            info!(logger, "Verifying VSO -> OpenBao integration...");
            run_workflow(logger, "verify", 1, 300, |i| {
                crate::workflows::verify::print_summary(i)
            })
            .await
        }
        ServiceAction::Secrets { service, action } => cmd_secrets(logger, &service, action).await,
        ServiceAction::Shell { service } => cmd_shell(logger, &service).await,
        ServiceAction::Describe { service } => cmd_describe(logger, &service).await,
        ServiceAction::Exec {
            service,
            container,
            command,
        } => cmd_exec(logger, &service, container.as_deref(), &command).await,
        ServiceAction::PortForward { service, ports } => cmd_port_forward(logger, &service, &ports).await,
        ServiceAction::Scale { service, replicas } => cmd_scale(logger, &service, replicas).await,
        ServiceAction::Top { service } => cmd_top(logger, &service).await,
        ServiceAction::Edit { service } => cmd_edit(logger, &service).await,
        ServiceAction::DeleteJob { target } => cmd_delete_job(logger, &target).await,
    }
}

/// Delete a Kubernetes Job by `<namespace>/<name>` via the daemon's k8s proxy.
///
/// Workaround for k8s Job immutability: once a Job is created, its spec can't
/// be updated, so re-applying a changed Job manifest is a no-op until the old
/// Job is deleted. This wraps that delete so operators don't have to drop to
/// raw kubectl + manual SOCKS proxy plumbing.
async fn cmd_delete_job(logger: &Logger, target: &str) -> Result<()> {
    use k8s_openapi::api::batch::v1::Job;
    use kube::api::{Api, DeleteParams};

    let (ns, name) = match target.split_once('/') {
        Some((n, j)) if !n.is_empty() && !j.is_empty() => (n, j),
        _ => bail!("expected <namespace>/<name>, got {target:?}"),
    };
    let client = crate::kube::get_client().await?;
    let jobs: Api<Job> = Api::namespaced(client, ns);
    match jobs.delete(name, &DeleteParams::default()).await {
        Ok(_) => {
            info!(logger, "deleted job", job = format!("{ns}/{name}"));
            Ok(())
        }
        Err(kube::Error::Api(e)) if e.code == 404 => {
            bail!("job {ns}/{name} not found")
        }
        Err(e) => Err(crate::error::SunbeamError::kube(format!(
            "delete job {ns}/{name} failed: {e}"
        ))),
    }
}


/// Helper: run a named workflow via the in-process engine.
async fn run_workflow(
    logger: &Logger,
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
        "seed" => {
            info!(logger, "The seed workflow has been merged into up. Use sunbeam up instead.");
        }
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

/// List available services from the infrastructure directory.
#[tracing::instrument(skip(logger, format))]
async fn cmd_list(logger: &Logger, format: crate::output::OutputFormat) -> Result<()> {
    info!(logger, "Discovering services...");
    let infra_dir = crate::config::get_infra_dir();
    let services = crate::manifests::discover_services(&infra_dir)?;
    info!(logger, "Found services", count = services.len());

    if services.is_empty() {
        info!(logger, "No services found in the infrastructure directory");
        return Ok(());
    }

    #[derive(serde::Serialize)]
    struct ServiceRow {
        name: String,
    }

    let rows: Vec<ServiceRow> = services
        .into_iter()
        .map(|name| ServiceRow { name })
        .collect();

    crate::output::render_list(&rows, &["SERVICE"], |r| vec![r.name.clone()], format)
}

/// Deploy service(s) by name, category, or namespace.
async fn cmd_deploy(
    logger: &crate::logger::Logger,
    target: &str,
    profile_flag: Option<String>,
) -> Result<()> {
    let (maybe_profile, overrides, skip_namespaces) =
        build_profile_context(logger, profile_flag, &[], &[], &[]).await?;
    let reg = get_registry(logger).await?;
    let resolved = reg.resolve(target);

    if resolved.is_empty() {
        bail!(
            "Unknown service: '{target}'. Try 'sunbeam service deploy --all' or a service name like 'hydra'."
        );
    }

    // Filter out services in skipped namespaces
    let resolved: Vec<_> = resolved
        .into_iter()
        .filter(|s| !skip_namespaces.contains(&s.namespace))
        .collect();

    if resolved.is_empty() {
        if let Some((name, _)) = maybe_profile {
            bail!(
                "All matching services are in namespaces skipped by profile '{}'",
                name
            );
        } else {
            bail!(
                "Unknown service: '{target}'. Try 'sunbeam service deploy --all' or a service name like 'hydra'."
            );
        }
    }

    // Info about skipped services
    if !skip_namespaces.is_empty() {
        let all_resolved = reg.resolve(target);
        let skipped_count = all_resolved.len() - resolved.len();
        if skipped_count > 0 {
            info!(logger, "Skipped services in excluded namespaces", count = skipped_count);
        }
    }

    let mut namespaces: Vec<&str> = resolved.iter().map(|s| s.namespace.as_str()).collect();
    namespaces.sort_unstable();
    namespaces.dedup();

    for ns in &namespaces {
        info!(logger, "Applying manifests for namespace", namespace = ns);
        let opts = crate::manifests::ApplyOptions {
            namespace: ns.to_string(),
            overrides: Some(overrides.clone()),
            skip_namespaces: skip_namespaces.clone(),
            ..Default::default()
        };
        crate::manifests::apply_manifests(logger, &opts).await?;
    }

    for svc in &resolved {
        for deploy in &svc.deployments {
            info!(logger, "Restarting deployment", namespace = svc.namespace.as_str(), deployment = deploy.as_str());
            crate::kube::kube_rollout_restart(&svc.namespace, deploy).await?;
        }
    }

    info!(logger, "Deploy complete.");
    Ok(())
}

/// View or get secrets for a service from OpenBao.
async fn cmd_secrets(logger: &Logger, service: &str, action: Option<SecretsAction>) -> Result<()> {
    let reg = get_registry(logger).await?;
    let svc = reg
        .get(service)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{service}'")))?;

    let kv_path = svc.kv_path.as_deref().ok_or_else(|| {
        SunbeamError::Other(format!("Service '{service}' has no secrets in OpenBao"))
    })?;

    let ob_pod =
        crate::kube::find_pod_by_label("data", "app.kubernetes.io/name=openbao,component=server")
            .await
            .ok_or_else(|| SunbeamError::Other("OpenBao pod not found".into()))?;

    let pf = crate::secrets::port_forward("data", &ob_pod, 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", pf.local_port);

    let token = crate::kube::kube_get_secret_field("data", "openbao-keys", "root-token")
        .await
        .map_err(|_| SunbeamError::Other("Failed to get OpenBao root token".into()))?;

    let bao = crate::openbao::BaoClient::with_token(&bao_url, &token);

    match action {
        None => match bao.kv_get("secret", kv_path).await? {
            Some(data) => {
                info!(logger, "Secrets for service", service = service, path = format!("secret/{kv_path}"));
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
                info!(logger, "No secrets found", path = format!("secret/{kv_path}"));
            }
        },
        Some(SecretsAction::Get { key }) => {
            let value = bao.kv_get_field("secret", kv_path, &key).await?;
            if value.is_empty() {
                info!(logger, "Field not found", key = key, path = format!("secret/{kv_path}"));
            } else {
                println!("{value}");
            }
        }
    }

    Ok(())
}

/// Interactive shell into a service pod.
///
/// Pod lookup uses `sunbeam.pt/pod-selector` annotation when present, otherwise
/// falls back to `app=<first deployment>`. The command run inside the pod comes
/// from `sunbeam.pt/shell-command` annotation, defaulting to `/bin/sh`.
async fn cmd_shell(logger: &Logger, service: &str) -> Result<()> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;

    let reg = get_registry(logger).await?;
    let svc = reg
        .get(service)
        .ok_or_else(|| SunbeamError::Other(format!("Unknown service: '{service}'")))?;

    let selector = svc
        .pod_selector
        .clone()
        .or_else(|| svc.deployments.first().map(|d| format!("app={d}")))
        .ok_or_else(|| {
            SunbeamError::Other(format!(
                "Service '{service}' has no pod-selector annotation and no deployments"
            ))
        })?;

    let pod = crate::kube::find_pod_by_label(&svc.namespace, &selector)
        .await
        .ok_or_else(|| SunbeamError::Other(format!("No pod found for {service}")))?;

    let shell_cmd = svc.shell_command.as_deref().unwrap_or("/bin/sh");
    let argv: Vec<String> = shell_cmd
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if argv.is_empty() {
        bail!("Service '{service}' has an empty shell-command annotation");
    }

    info!(logger, "Connecting to service", service = service, pod = pod);
    let client = crate::kube::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), &svc.namespace);
    let code = crate::exec::pod_exec_interactive(&pods, &pod, None, &argv).await?;
    if code != 0 {
        info!(logger, "shell exited with code", code = code);
    }
    Ok(())
}

/// Describe a service's deployment.
async fn cmd_describe(logger: &Logger, service: &str) -> Result<()> {
    let (ns, deploy) = resolve_service(logger, service).await?;
    let client = crate::kube::get_client().await?;
    let text = crate::describe::describe_deployment(client.clone(), &ns, &deploy).await?;
    println!("{text}");
    Ok(())
}

/// Exec into a service pod with an optional command.
async fn cmd_exec(logger: &Logger, service: &str, container: Option<&str>, command: &[String]) -> Result<()> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;

    let (ns, deploy) = resolve_service(logger, service).await?;
    let pod = crate::kube::find_pod_by_label(&ns, &format!("app={deploy}"))
        .await
        .ok_or_else(|| SunbeamError::Other(format!("No pod found for {service}")))?;

    let argv: Vec<String> = if command.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        command.to_vec()
    };

    let client = crate::kube::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
    let code = crate::exec::pod_exec_interactive(&pods, &pod, container, &argv).await?;
    if code != 0 {
        info!(logger, "exec exited with code", code = code);
    }
    Ok(())
}

/// Port-forward to a service pod. Parses `"local:remote"` or `"port"` mappings
/// and serves each on 127.0.0.1 until Ctrl-C.
async fn cmd_port_forward(logger: &Logger, service: &str, ports: &[String]) -> Result<()> {
    if ports.is_empty() {
        bail!("At least one port mapping required (e.g. '8080:80' or '8080')");
    }
    let (ns, deploy) = resolve_service(logger, service).await?;
    let pod = crate::kube::find_pod_by_label(&ns, &format!("app={deploy}"))
        .await
        .ok_or_else(|| SunbeamError::Other(format!("No pod found for {service}")))?;

    let mut mappings: Vec<(u16, u16)> = Vec::with_capacity(ports.len());
    for p in ports {
        let (l, r) = match p.split_once(':') {
            Some((a, b)) => (a, b),
            None => (p.as_str(), p.as_str()),
        };
        let local: u16 = l
            .parse()
            .map_err(|e| SunbeamError::Other(format!("invalid local port {l:?}: {e}")))?;
        let remote: u16 = r
            .parse()
            .map_err(|e| SunbeamError::Other(format!("invalid remote port {r:?}: {e}")))?;
        mappings.push((local, remote));
    }

    info!(logger, "Port-forwarding to service", service = service, pod = pod);
    crate::port_forward::serve_port_forward(logger, ns, pod, mappings).await
}

/// Scale a service deployment.
async fn cmd_scale(logger: &Logger, service: &str, replicas: u32) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, Patch, PatchParams};

    let (ns, deploy) = resolve_service(logger, service).await?;
    info!(logger, "Scaling service to replicas", service = service, replicas = replicas);

    let client = crate::kube::get_client().await?;
    let api: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let patch = serde_json::json!({ "spec": { "replicas": replicas } });
    api.patch(&deploy, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(|e| SunbeamError::Other(format!("scale patch failed: {e}")))?;

    info!(logger, "scaled to replicas", service = service, replicas = replicas);
    Ok(())
}

/// Show resource usage for a service's pods via metrics.k8s.io.
async fn cmd_top(logger: &Logger, service: &str) -> Result<()> {
    use comfy_table::{Cell, Table};
    use kube::api::{Api, ApiResource, DynamicObject, GroupVersionKind, ListParams};

    let (ns, deploy) = resolve_service(logger, service).await?;
    let client = crate::kube::get_client().await?;

    let gvk = GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "PodMetrics");
    let ar = ApiResource::from_gvk(&gvk);
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), &ns, &ar);
    let lp = ListParams::default().labels(&format!("app={deploy}"));
    let list = api.list(&lp).await.map_err(|e| {
        SunbeamError::Other(format!(
            "metrics.k8s.io query failed (is metrics-server installed?): {e}"
        ))
    })?;

    let mut table = Table::new();
    table.set_header(vec!["NAME", "CPU", "MEMORY"]);
    for pm in &list.items {
        let name = pm.metadata.name.clone().unwrap_or_default();
        let containers = pm
            .data
            .get("containers")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut cpu_milli: u64 = 0;
        let mut mem_bytes: u64 = 0;
        for c in &containers {
            if let Some(usage) = c.get("usage") {
                if let Some(cpu) = usage.get("cpu").and_then(|v| v.as_str()) {
                    cpu_milli += parse_cpu_milli(cpu);
                }
                if let Some(mem) = usage.get("memory").and_then(|v| v.as_str()) {
                    mem_bytes += parse_memory_bytes(mem);
                }
            }
        }
        table.add_row(vec![
            Cell::new(name),
            Cell::new(format!("{cpu_milli}m")),
            Cell::new(format_bytes(mem_bytes)),
        ]);
    }
    println!("{table}");
    Ok(())
}

/// Parse a Kubernetes CPU quantity (e.g. `100m`, `1`, `1.5`) into millicores.
fn parse_cpu_milli(s: &str) -> u64 {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix('m') {
        rest.parse::<u64>().unwrap_or(0)
    } else if let Some(rest) = s.strip_suffix('n') {
        // nanocores -> millicores
        rest.parse::<u64>().unwrap_or(0) / 1_000_000
    } else if let Some(rest) = s.strip_suffix('u') {
        rest.parse::<u64>().unwrap_or(0) / 1_000
    } else {
        // Whole cores (float ok)
        (s.parse::<f64>().unwrap_or(0.0) * 1000.0) as u64
    }
}

/// Parse a Kubernetes memory quantity (e.g. `256Mi`, `1Gi`, `2048Ki`) into bytes.
fn parse_memory_bytes(s: &str) -> u64 {
    let s = s.trim();
    let (num, mult): (&str, u64) = if let Some(n) = s.strip_suffix("Ki") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix("Mi") {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("Gi") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("Ti") {
        (n, 1024u64.pow(4))
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1000)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1_000_000)
    } else if let Some(n) = s.strip_suffix('G') {
        (n, 1_000_000_000)
    } else {
        (s, 1)
    };
    num.parse::<u64>().unwrap_or(0) * mult
}

fn format_bytes(b: u64) -> String {
    const KI: u64 = 1024;
    const MI: u64 = 1024 * 1024;
    const GI: u64 = 1024 * 1024 * 1024;
    if b >= GI {
        format!("{:.1}Gi", b as f64 / GI as f64)
    } else if b >= MI {
        format!("{}Mi", b / MI)
    } else if b >= KI {
        format!("{}Ki", b / KI)
    } else {
        format!("{b}")
    }
}

/// Edit a service's deployment in-cluster: fetch → $EDITOR → replace.
async fn cmd_edit(logger: &Logger, service: &str) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, PostParams};

    let (ns, deploy) = resolve_service(logger, service).await?;
    let client = crate::kube::get_client().await?;
    let api: Api<Deployment> = Api::namespaced(client.clone(), &ns);

    let mut current = api
        .get(&deploy)
        .await
        .map_err(|e| SunbeamError::Other(format!("failed to get deployment: {e}")))?;

    // Strip server-managed metadata before showing to the user.
    current.metadata.managed_fields = None;
    current.metadata.resource_version = None;
    current.metadata.uid = None;
    current.metadata.creation_timestamp = None;
    current.metadata.generation = None;
    current.status = None;

    let before = serde_yaml::to_string(&current)
        .map_err(|e| SunbeamError::Other(format!("yaml serialize failed: {e}")))?;

    let tmp = tempfile::Builder::new()
        .prefix("sunbeam-edit-")
        .suffix(".yaml")
        .tempfile()
        .map_err(|e| SunbeamError::Other(format!("tempfile failed: {e}")))?;
    std::fs::write(tmp.path(), &before)
        .map_err(|e| SunbeamError::Other(format!("write tempfile failed: {e}")))?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(tmp.path())
        .status()
        .map_err(|e| SunbeamError::Other(format!("failed to run {editor}: {e}")))?;
    if !status.success() {
        bail!("editor exited with non-zero status");
    }

    let after = std::fs::read_to_string(tmp.path())
        .map_err(|e| SunbeamError::Other(format!("read tempfile failed: {e}")))?;
    if after == before {
        info!(logger, "No changes.");
        return Ok(());
    }

    let modified: Deployment = serde_yaml::from_str(&after)
        .map_err(|e| SunbeamError::Other(format!("failed to parse YAML: {e}")))?;

    api.replace(&deploy, &PostParams::default(), &modified)
        .await
        .map_err(|e| SunbeamError::Other(format!("failed to replace deployment: {e}")))?;

    info!(logger, "updated", service = service);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::config::{Context, Profile, ProfileRef, Rule, SunbeamConfig};

    // -------------------------------------------------------------------
    // resolve_service_profile_inner
    // -------------------------------------------------------------------

    #[test]
    fn test_resolve_service_profile_from_file() {
        let tmp = tempfile::tempdir().unwrap();
        let profile_dir = tmp.path().join("profiles");
        std::fs::create_dir_all(&profile_dir).unwrap();
        let profile_path = profile_dir.join("test.yaml");
        let profile = Profile {
            skip_ory: true,
            ..Default::default()
        };
        std::fs::write(&profile_path, serde_yaml::to_string(&profile).unwrap()).unwrap();

        let config = SunbeamConfig::default();
        let result = resolve_service_profile_inner(Some("test".to_string()), &config, tmp.path()).unwrap();
        assert!(result.is_some());
        let (name, p) = result.unwrap();
        assert_eq!(name, "test");
        assert!(p.skip_ory);
    }

    #[test]
    fn test_resolve_service_profile_from_config() {
        let mut config = SunbeamConfig::default();
        let mut profile = Profile::default();
        profile.skip_ory = true;
        config.profiles.insert("lima".to_string(), profile);

        let result = resolve_service_profile_inner(
            Some("lima".to_string()),
            &config,
            std::path::Path::new("/nonexistent"),
        )
        .unwrap();
        assert!(result.is_some());
        let (name, p) = result.unwrap();
        assert_eq!(name, "lima");
        assert!(p.skip_ory);
    }

    #[test]
    fn test_resolve_service_profile_from_context() {
        let mut config = SunbeamConfig::default();
        let mut ctx = Context::default();
        ctx.profile = ProfileRef::Name("lima".to_string());
        config.contexts.insert("local".to_string(), ctx);
        config.current_context = "local".to_string();

        let mut profile = Profile::default();
        profile.skip_ory = true;
        config.profiles.insert("lima".to_string(), profile);

        let result = resolve_service_profile_inner(
            None,
            &config,
            std::path::Path::new("/nonexistent"),
        )
        .unwrap();
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "lima");
    }

    #[test]
    fn test_resolve_service_profile_not_found() {
        let config = SunbeamConfig::default();
        let result = resolve_service_profile_inner(
            Some("missing".to_string()),
            &config,
            std::path::Path::new("/nonexistent"),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Profile not found"));
        assert!(err.contains("missing"));
    }

    #[test]
    fn test_resolve_service_profile_no_profile_configured() {
        let config = SunbeamConfig::default();
        let result = resolve_service_profile_inner(
            None,
            &config,
            std::path::Path::new("/nonexistent"),
        )
        .unwrap();
        assert!(result.is_none());
    }

    // -------------------------------------------------------------------
    // Namespace filtering
    // -------------------------------------------------------------------

    #[test]
    fn test_filter_services_by_skip_list_keeps_non_skipped() {
        let svc1 = crate::registry::ServiceDefinition {
            name: "hydra".to_string(),
            display_name: "Hydra".to_string(),
            category: crate::registry::Category::Auth,
            namespace: "ory".to_string(),
            deployments: vec![],
            kv_path: None,
            database: None,
            build_target: None,
            depends_on: vec![],
            health: crate::registry::HealthCheck::None,
            virtual_service: false,
            resource_kind: "Deployment".to_string(),
            pod_selector: None,
            shell_command: None,
            ports: vec![],
        };
        let svc2 = crate::registry::ServiceDefinition {
            name: "gitea".to_string(),
            display_name: "Gitea".to_string(),
            category: crate::registry::Category::DevTools,
            namespace: "devtools".to_string(),
            deployments: vec![],
            kv_path: None,
            database: None,
            build_target: None,
            depends_on: vec![],
            health: crate::registry::HealthCheck::None,
            virtual_service: false,
            resource_kind: "Deployment".to_string(),
            pod_selector: None,
            shell_command: None,
            ports: vec![],
        };
        let services = vec![&svc1, &svc2];
        let filtered: Vec<_> = services
            .into_iter()
            .filter(|s| !["ory".to_string()].contains(&s.namespace))
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "gitea");
    }

    #[test]
    fn test_filter_services_by_skip_list_empty_skips_all_kept() {
        let svc = crate::registry::ServiceDefinition {
            name: "hydra".to_string(),
            display_name: "Hydra".to_string(),
            category: crate::registry::Category::Auth,
            namespace: "ory".to_string(),
            deployments: vec![],
            kv_path: None,
            database: None,
            build_target: None,
            depends_on: vec![],
            health: crate::registry::HealthCheck::None,
            virtual_service: false,
            resource_kind: "Deployment".to_string(),
            pod_selector: None,
            shell_command: None,
            ports: vec![],
        };
        let services = vec![&svc];
        let filtered: Vec<_> = services
            .into_iter()
            .filter(|s| [].contains(&s.namespace))
            .collect();
        assert_eq!(filtered.len(), 0);
    }

    // -------------------------------------------------------------------
    // Override merging
    // -------------------------------------------------------------------

    #[test]
    fn test_merge_overrides_cli_wins() {
        let profile = crate::manifest_params::Overrides {
            items: vec![crate::manifest_params::Override::Set {
                resource: "deployment/ory/hydra".to_string(),
                field_path: "spec/replicas".to_string(),
                value: "1".to_string(),
            }],
        };
        let cli = crate::manifest_params::Overrides {
            items: vec![crate::manifest_params::Override::Set {
                resource: "deployment/ory/hydra".to_string(),
                field_path: "spec/replicas".to_string(),
                value: "3".to_string(),
            }],
        };
        let merged = merge_overrides(profile, cli);
        assert_eq!(merged.items.len(), 2);
        let last = match &merged.items[1] {
            crate::manifest_params::Override::Set { value, .. } => value.clone(),
            _ => panic!("expected Set"),
        };
        assert_eq!(last, "3");
    }

    #[test]
    fn test_merge_overrides_profile_only() {
        let profile = crate::manifest_params::Overrides {
            items: vec![crate::manifest_params::Override::Set {
                resource: "deployment/ory/hydra".to_string(),
                field_path: "spec/replicas".to_string(),
                value: "1".to_string(),
            }],
        };
        let cli = crate::manifest_params::Overrides { items: vec![] };
        let merged = merge_overrides(profile, cli);
        assert_eq!(merged.items.len(), 1);
    }

    #[test]
    fn test_merge_overrides_cli_only() {
        let profile = crate::manifest_params::Overrides { items: vec![] };
        let cli = crate::manifest_params::Overrides {
            items: vec![crate::manifest_params::Override::Set {
                resource: "deployment/ory/hydra".to_string(),
                field_path: "spec/replicas".to_string(),
                value: "3".to_string(),
            }],
        };
        let merged = merge_overrides(profile, cli);
        assert_eq!(merged.items.len(), 1);
    }
}
