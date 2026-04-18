//! Service-oriented CLI commands: deploy, secrets, shell, describe, exec, port-forward, scale, top, edit.
//!
//! These commands use the service registry for name resolution and delegate to
//! kubectl for interactive operations.

use crate::cli::{SecretsAction, ServiceAction, TransitAction};
use crate::error::{Result, SunbeamError};
use crate::output::{ok, step, warn};
use crate::registry::{self, ServiceRegistry};

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
        ServiceAction::Status { target } => crate::services::cmd_status(target.as_deref()).await,
        ServiceAction::Logs { target, follow } => crate::services::cmd_logs(&target, follow).await,
        ServiceAction::Get { target, output } => crate::services::cmd_get(&target, &output).await,
        ServiceAction::Restart { target } => crate::services::cmd_restart(target.as_deref()).await,
        ServiceAction::Check { target } => crate::checks::cmd_check(target.as_deref()).await,
        ServiceAction::Deploy { target, all } => match target {
            Some(t) if !all => cmd_deploy(&t, domain, email).await,
            _ => crate::manifests::cmd_apply(domain, email, "").await,
        },
        ServiceAction::Apply {
            namespace,
            apply_all,
            domain: apply_domain,
            email: apply_email,
        } => {
            let d = if apply_domain.is_empty() {
                domain.to_string()
            } else {
                apply_domain
            };
            let e = if apply_email.is_empty() {
                email.to_string()
            } else {
                apply_email
            };
            let ns = namespace.unwrap_or_default();

            if ns.is_empty() && !apply_all {
                crate::output::warn("This will apply ALL namespaces.");
                eprint!("  Continue? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            crate::manifests::cmd_apply(&d, &e, &ns).await
        }
        ServiceAction::Seed => {
            crate::output::step("Seeding secrets (workflow engine)...");
            run_workflow("seed", 2, 900, crate::workflows::seed::print_summary).await
        }
        ServiceAction::Verify => {
            crate::output::step("Verifying VSO -> OpenBao integration...");
            run_workflow("verify", 1, 300, |i| {
                crate::workflows::verify::print_summary(i)
            })
            .await
        }
        ServiceAction::Secrets { service, action } => cmd_secrets(&service, action).await,
        ServiceAction::Shell { service } => cmd_shell(&service).await,
        ServiceAction::Describe { service } => cmd_describe(&service).await,
        ServiceAction::Exec {
            service,
            container,
            command,
        } => cmd_exec(&service, container.as_deref(), &command).await,
        ServiceAction::PortForward { service, ports } => cmd_port_forward(&service, &ports).await,
        ServiceAction::Scale { service, replicas } => cmd_scale(&service, replicas).await,
        ServiceAction::Top { service } => cmd_top(&service).await,
        ServiceAction::Edit { service } => cmd_edit(&service).await,
        ServiceAction::Transit { action } => cmd_transit(action).await,
    }
}

/// Manage OpenBao Transit: enable mounts, create keys, read public key metadata.
async fn cmd_transit(action: TransitAction) -> Result<()> {
    let ob_pod =
        crate::kube::find_pod_by_label("data", "app.kubernetes.io/name=openbao,component=server")
            .await
            .ok_or_else(|| SunbeamError::Other("OpenBao pod not found".into()))?;

    let _pf = crate::secrets::port_forward("data", &ob_pod, 8200).await?;
    let bao_url = format!("http://127.0.0.1:{}", _pf.local_port);

    let token = crate::kube::kube_get_secret_field("data", "openbao-keys", "root-token")
        .await
        .map_err(|_| SunbeamError::Other("Failed to get OpenBao root token".into()))?;

    let bao = crate::openbao::BaoClient::with_token(&bao_url, &token);

    match action {
        TransitAction::Enable { mount } => {
            let path = mount.trim_matches('/');
            step(&format!("Enabling transit engine at {path}..."));
            bao.enable_secrets_engine(path, "transit").await?;
            ok(&format!("Transit engine ready at {path}/"));
        }
        TransitAction::CreateKey {
            mount,
            name,
            key_type,
        } => {
            let mount = mount.trim_matches('/');
            let key_path = format!("{mount}/keys/{name}");

            if let Some(existing) = bao.read(&key_path).await? {
                ok(&format!("Key {key_path} already exists."));
                if let Some(t) = existing
                    .get("data")
                    .and_then(|d| d.get("type"))
                    .and_then(|v| v.as_str())
                    && t != key_type
                {
                    warn(&format!(
                        "Existing key type is {t}, requested {key_type} — leaving as-is."
                    ));
                }
                return Ok(());
            }

            step(&format!("Creating {key_type} key at {key_path}..."));
            bao.write(&key_path, &serde_json::json!({ "type": key_type }))
                .await?;
            ok(&format!("Key {key_path} created."));
        }
        TransitAction::ReadKey { mount, name } => {
            let mount = mount.trim_matches('/');
            let key_path = format!("{mount}/keys/{name}");
            match bao.read(&key_path).await? {
                Some(value) => {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                None => {
                    warn(&format!("Key {key_path} not found."));
                }
            }
        }
    }

    Ok(())
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
        bail!(
            "Unknown service: '{target}'. Try 'sunbeam service deploy --all' or a service name like 'hydra'."
        );
    }

    let mut namespaces: Vec<&str> = resolved.iter().map(|s| s.namespace.as_str()).collect();
    namespaces.sort_unstable();
    namespaces.dedup();

    for ns in &namespaces {
        step(&format!("Applying manifests for {ns}..."));
        crate::manifests::cmd_apply(domain, email, ns).await?;
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
        },
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
/// Pod lookup uses `sunbeam.pt/pod-selector` annotation when present, otherwise
/// falls back to `app=<first deployment>`. The command run inside the pod comes
/// from `sunbeam.pt/shell-command` annotation, defaulting to `/bin/sh`.
async fn cmd_shell(service: &str) -> Result<()> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;

    let reg = get_registry().await?;
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

    step(&format!("Connecting to {service} ({pod})..."));
    let client = crate::kube::get_client().await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), &svc.namespace);
    let code = crate::exec::pod_exec_interactive(&pods, &pod, None, &argv).await?;
    if code != 0 {
        warn(&format!("shell exited with code {code}"));
    }
    Ok(())
}

/// Describe a service's deployment.
async fn cmd_describe(service: &str) -> Result<()> {
    let (ns, deploy) = resolve_service(service).await?;
    let client = crate::kube::get_client().await?;
    let text = crate::describe::describe_deployment(client.clone(), &ns, &deploy).await?;
    println!("{text}");
    Ok(())
}

/// Exec into a service pod with an optional command.
async fn cmd_exec(service: &str, container: Option<&str>, command: &[String]) -> Result<()> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;

    let (ns, deploy) = resolve_service(service).await?;
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
        warn(&format!("exec exited with code {code}"));
    }
    Ok(())
}

/// Port-forward to a service pod. Parses `"local:remote"` or `"port"` mappings
/// and serves each on 127.0.0.1 until Ctrl-C.
async fn cmd_port_forward(service: &str, ports: &[String]) -> Result<()> {
    if ports.is_empty() {
        bail!("At least one port mapping required (e.g. '8080:80' or '8080')");
    }
    let (ns, deploy) = resolve_service(service).await?;
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

    step(&format!("Port-forwarding to {service} ({pod})..."));
    crate::port_forward::serve_port_forward(ns, pod, mappings).await
}

/// Scale a service deployment.
async fn cmd_scale(service: &str, replicas: u32) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, Patch, PatchParams};

    let (ns, deploy) = resolve_service(service).await?;
    step(&format!("Scaling {service} to {replicas} replica(s)..."));

    let client = crate::kube::get_client().await?;
    let api: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let patch = serde_json::json!({ "spec": { "replicas": replicas } });
    api.patch(&deploy, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(|e| SunbeamError::Other(format!("scale patch failed: {e}")))?;

    ok(&format!("{service} scaled to {replicas}."));
    Ok(())
}

/// Show resource usage for a service's pods via metrics.k8s.io.
async fn cmd_top(service: &str) -> Result<()> {
    use comfy_table::{Cell, Table};
    use kube::api::{Api, ApiResource, DynamicObject, GroupVersionKind, ListParams};

    let (ns, deploy) = resolve_service(service).await?;
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
async fn cmd_edit(service: &str) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, PostParams};

    let (ns, deploy) = resolve_service(service).await?;
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
        ok("No changes.");
        return Ok(());
    }

    let modified: Deployment = serde_yaml::from_str(&after)
        .map_err(|e| SunbeamError::Other(format!("failed to parse YAML: {e}")))?;

    api.replace(&deploy, &PostParams::default(), &modified)
        .await
        .map_err(|e| SunbeamError::Other(format!("failed to replace deployment: {e}")))?;

    ok(&format!("{service} updated."));
    Ok(())
}
