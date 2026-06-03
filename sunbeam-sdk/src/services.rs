//! Service management — status, logs, restart.

use crate::error::{Result, SunbeamError};
use crate::kube::{get_client, kube_rollout_restart, parse_target};

use crate::registry::{self, Category, ServiceRegistry};
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::api::{Api, DynamicObject, ListParams, LogParams};

// ---------------------------------------------------------------------------
// Registry helper
// ---------------------------------------------------------------------------

/// Discover the service registry from the cluster.
async fn get_registry(logger: &crate::logger::Logger) -> Result<ServiceRegistry> {
    let client = get_client().await?;
    registry::discover(logger, &client)
        .await
        .map_err(|e| SunbeamError::Other(format!("service discovery failed: {e}")))
}

// ---------------------------------------------------------------------------
// Status helpers
// ---------------------------------------------------------------------------

/// Parsed pod row for display.
struct PodRow {
    ns: String,
    name: String,
    ready: String,
    status: String,
    /// True when this pod was found via namespace-wide fallback rather than a
    /// label match. Displayed as "(unlabeled)" in the status table.
    unlabeled: bool,
}

fn icon_for_status(status: &str) -> &'static str {
    match status {
        "Running" | "Completed" | "Succeeded" => "\u{2713}",
        "Pending" => "\u{25cb}",
        "Failed" => "\u{2717}",
        _ => "?",
    }
}

fn pod_phase(pod: &Pod) -> String {
    pod.status
        .as_ref()
        .and_then(|s| s.phase.clone())
        .unwrap_or_else(|| "Unknown".to_string())
}

fn pod_ready_str(pod: &Pod) -> String {
    let cs = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref());
    match cs {
        Some(cs) => {
            let total = cs.len();
            let ready = cs.iter().filter(|c| c.ready).count();
            format!("{ready}/{total}")
        }
        None => "0/0".to_string(),
    }
}

// ---------------------------------------------------------------------------
// VSO sync status
// ---------------------------------------------------------------------------

async fn vso_sync_status() -> Result<()> {
    tracing::info!("VSO secret sync status...");

    let client = get_client().await?;
    let mut all_ok = true;

    // --- VaultStaticSecrets ---
    {
        let ar = kube::api::ApiResource {
            group: "secrets.hashicorp.com".into(),
            version: "v1beta1".into(),
            api_version: "secrets.hashicorp.com/v1beta1".into(),
            kind: "VaultStaticSecret".into(),
            plural: "vaultstaticsecrets".into(),
        };

        let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
        let list = api.list(&ListParams::default()).await;

        if let Ok(list) = list {
            // Group by namespace and sort
            let mut grouped: std::collections::BTreeMap<String, Vec<(String, bool)>> =
                std::collections::BTreeMap::new();
            for obj in &list.items {
                let ns = obj.namespace().unwrap_or_default();
                let name = obj.name_any();
                let mac = obj
                    .data
                    .get("status")
                    .and_then(|s| s.get("secretMAC"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let synced = !mac.is_empty() && mac != "<none>";
                if !synced {
                    all_ok = false;
                }
                grouped.entry(ns).or_default().push((name, synced));
            }
            for (ns, mut items) in grouped {
                println!("  {ns} (VSS):");
                items.sort();
                for (name, synced) in items {
                    let icon = if synced { "\u{2713}" } else { "\u{2717}" };
                    println!("    {icon} {name}");
                }
            }
        }
    }

    // --- VaultDynamicSecrets ---
    {
        let ar = kube::api::ApiResource {
            group: "secrets.hashicorp.com".into(),
            version: "v1beta1".into(),
            api_version: "secrets.hashicorp.com/v1beta1".into(),
            kind: "VaultDynamicSecret".into(),
            plural: "vaultdynamicsecrets".into(),
        };

        let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
        let list = api.list(&ListParams::default()).await;

        if let Ok(list) = list {
            let mut grouped: std::collections::BTreeMap<String, Vec<(String, bool)>> =
                std::collections::BTreeMap::new();
            for obj in &list.items {
                let ns = obj.namespace().unwrap_or_default();
                let name = obj.name_any();
                let renewed = obj
                    .data
                    .get("status")
                    .and_then(|s| s.get("lastRenewalTime"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let synced = !renewed.is_empty() && renewed != "0" && renewed != "<none>";
                if !synced {
                    all_ok = false;
                }
                grouped.entry(ns).or_default().push((name, synced));
            }
            for (ns, mut items) in grouped {
                println!("  {ns} (VDS):");
                items.sort();
                for (name, synced) in items {
                    let icon = if synced { "\u{2713}" } else { "\u{2717}" };
                    println!("    {icon} {name}");
                }
            }
        }
    }

    println!();
    if all_ok {
        tracing::info!("All VSO secrets synced.");
    } else {
        tracing::warn!("Some VSO secrets are not synced.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public commands
// ---------------------------------------------------------------------------

/// Show pod health, optionally filtered by service name, category, namespace,
/// or legacy namespace/service syntax.
#[tracing::instrument(skip(logger, target))]
pub async fn cmd_status(logger: &crate::logger::Logger, target: Option<&str>) -> Result<()> {
    tracing::info!("Pod health across all namespaces...");

    let client = get_client().await?;
    let reg = get_registry(logger).await?;

    let mut pods: Vec<PodRow> = Vec::new();

    match target {
        None => {
            // All managed namespaces (derived from registry)
            let namespaces = reg.namespaces();
            let ns_set: std::collections::HashSet<&str> = namespaces.iter().copied().collect();
            for ns in &namespaces {
                let api: Api<Pod> = Api::namespaced(client.clone(), ns);
                let lp = ListParams::default();
                if let Ok(list) = api.list(&lp).await {
                    for pod in list.items {
                        let pod_ns = pod.namespace().unwrap_or_default();
                        if !ns_set.contains(pod_ns.as_str()) {
                            continue;
                        }
                        pods.push(PodRow {
                            ns: pod_ns,
                            name: pod.name_any(),
                            ready: pod_ready_str(&pod),
                            status: pod_phase(&pod),
                            unlabeled: false,
                        });
                    }
                }
            }
        }
        Some(input) => {
            // Try registry resolution first
            let resolved = reg.resolve(input);
            if !resolved.is_empty() {
                // Collect unique namespaces from resolved services
                let mut namespaces: Vec<&str> =
                    resolved.iter().map(|s| s.namespace.as_str()).collect();
                namespaces.sort_unstable();
                namespaces.dedup();

                // Collect deployment names for label filtering
                let deploy_names: std::collections::HashSet<&str> = resolved
                    .iter()
                    .flat_map(|s| s.deployments.iter().map(|d| d.as_str()))
                    .collect();

                for ns in &namespaces {
                    let api: Api<Pod> = Api::namespaced(client.clone(), ns);
                    if deploy_names.is_empty() {
                        // Services with no deployments: show all pods in namespace
                        let lp = ListParams::default();
                        if let Ok(list) = api.list(&lp).await {
                            for pod in list.items {
                                pods.push(PodRow {
                                    ns: ns.to_string(),
                                    name: pod.name_any(),
                                    ready: pod_ready_str(&pod),
                                    status: pod_phase(&pod),
                                    unlabeled: false,
                                });
                            }
                        }
                    } else {
                        // Filter by app label for each deployment
                        for deploy in &deploy_names {
                            let lp = ListParams::default().labels(&format!("app={deploy}"));
                            if let Ok(list) = api.list(&lp).await {
                                for pod in list.items {
                                    pods.push(PodRow {
                                        ns: ns.to_string(),
                                        name: pod.name_any(),
                                        ready: pod_ready_str(&pod),
                                        status: pod_phase(&pod),
                                        unlabeled: false,
                                    });
                                }
                            }
                        }
                    }
                }
            } else {
                // Fallback: parse as namespace or namespace/name
                let (ns_filter, svc_filter) = parse_target(Some(input))?;
                match (ns_filter, svc_filter) {
                    (Some(ns), None) => {
                        let api: Api<Pod> = Api::namespaced(client.clone(), ns);
                        let lp = ListParams::default();
                        if let Ok(list) = api.list(&lp).await {
                            for pod in list.items {
                                pods.push(PodRow {
                                    ns: ns.to_string(),
                                    name: pod.name_any(),
                                    ready: pod_ready_str(&pod),
                                    status: pod_phase(&pod),
                                    unlabeled: false,
                                });
                            }
                        }
                    }
                    (Some(ns), Some(svc)) => {
                        let api: Api<Pod> = Api::namespaced(client.clone(), ns);
                        // Try label-based query first.
                        let lp = ListParams::default().labels(&format!("app={svc}"));
                        let labeled_list = api.list(&lp).await.ok();
                        let labeled_hit = labeled_list
                            .as_ref()
                            .map(|l| !l.items.is_empty())
                            .unwrap_or(false);

                        if labeled_hit {
                            for pod in labeled_list.unwrap().items {
                                pods.push(PodRow {
                                    ns: ns.to_string(),
                                    name: pod.name_any(),
                                    ready: pod_ready_str(&pod),
                                    status: pod_phase(&pod),
                                    unlabeled: false,
                                });
                            }
                        } else {
                            // No labeled pods — fall back to all pods in the namespace.
                            let lp_all = ListParams::default();
                            if let Ok(list) = api.list(&lp_all).await {
                                for pod in list.items {
                                    pods.push(PodRow {
                                        ns: ns.to_string(),
                                        name: pod.name_any(),
                                        ready: pod_ready_str(&pod),
                                        status: pod_phase(&pod),
                                        unlabeled: true,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if pods.is_empty() {
        tracing::warn!("No pods found in managed namespaces.");
        return Ok(());
    }

    pods.sort_by(|a, b| (&a.ns, &a.name).cmp(&(&b.ns, &b.name)));

    let mut all_ok = true;
    let mut cur_ns: Option<&str> = None;
    for row in &pods {
        if cur_ns != Some(&row.ns) {
            println!("  {}:", row.ns);
            cur_ns = Some(&row.ns);
        }
        let icon = icon_for_status(&row.status);

        let mut unhealthy = !matches!(row.status.as_str(), "Running" | "Completed" | "Succeeded");
        // For Running pods, check ready ratio
        if !unhealthy && row.status == "Running" && row.ready.contains('/') {
            let parts: Vec<&str> = row.ready.split('/').collect();
            if parts.len() == 2 && parts[0] != parts[1] {
                unhealthy = true;
            }
        }
        if unhealthy {
            all_ok = false;
        }
        let display_name = if row.unlabeled {
            format!("{} (unlabeled)", row.name)
        } else {
            row.name.clone()
        };
        println!(
            "    {icon} {:<50} {:<6} {}",
            display_name, row.ready, row.status
        );
    }

    println!();
    if all_ok {
        tracing::info!("All pods healthy.");
    } else {
        tracing::warn!("Some pods are not ready.");
    }

    vso_sync_status().await?;
    Ok(())
}

/// Stream logs for a service. Accepts a service name (e.g. "hydra") or legacy
/// namespace/name syntax (e.g. "ory/kratos").
#[tracing::instrument(skip(logger, target))]
pub async fn cmd_logs(logger: &crate::logger::Logger, target: &str, follow: bool) -> Result<()> {
    // Try registry first for exact service name match
    let reg = get_registry(logger).await?;
    let (ns, name) = if let Some(svc) = reg.get(target) {
        if svc.deployments.is_empty() {
            bail!("{target} has no deployments to show logs for");
        }
        (svc.namespace.as_str(), svc.deployments[0].as_str())
    } else {
        // Fallback: parse as namespace/name
        let (ns_opt, name_opt) = parse_target(Some(target))?;
        let ns = ns_opt.unwrap_or("");
        let name = match name_opt {
            Some(n) => n,
            None => bail!(
                "No service '{target}' found in registry. Use namespace/name syntax \
                 (e.g. 'ory/kratos') or a known service name."
            ),
        };
        (ns, name)
    };

    let client = get_client().await?;
    let api: Api<Pod> = Api::namespaced(client.clone(), ns);

    // Find pods matching the app label
    let lp = ListParams::default().labels(&format!("app={name}"));
    let pod_list = api.list(&lp).await?;

    if pod_list.items.is_empty() {
        bail!("No pods found for {ns}/{name}");
    }

    if follow {
        // Stream logs from the first matching pod
        let pod_name = pod_list.items[0].name_any();
        let lp = LogParams {
            follow: true,
            tail_lines: Some(100),
            ..Default::default()
        };

        // log_stream returns a futures::AsyncBufRead — use the futures crate to read it
        use futures::AsyncBufReadExt;
        let stream = api.log_stream(&pod_name, &lp).await?;
        let reader = futures::io::BufReader::new(stream);
        let mut lines = reader.lines();
        use futures::StreamExt;
        while let Some(line) = lines.next().await {
            match line {
                Ok(line) => println!("{line}"),
                Err(e) => {
                    tracing::warn!("Log stream error: {e}");
                    break;
                }
            }
        }
    } else {
        // Print logs from all matching pods
        for pod in &pod_list.items {
            let pod_name = pod.name_any();
            let lp = LogParams {
                tail_lines: Some(100),
                ..Default::default()
            };

            match api.logs(&pod_name, &lp).await {
                Ok(logs) => print!("{logs}"),
                Err(e) => tracing::warn!("Failed to get logs for {pod_name}: {e}"),
            }
        }
    }

    Ok(())
}

/// Print raw pod output in YAML or JSON format.
#[tracing::instrument(skip(logger, target, output))]
pub async fn cmd_get(logger: &crate::logger::Logger, target: &str, output: &str) -> Result<()> {
    let (ns_opt, name_opt) = parse_target(Some(target))?;
    let ns = match ns_opt {
        Some(n) if !n.is_empty() => n,
        _ => bail!("get requires namespace/name, e.g. 'sunbeam get ory/kratos-abc'"),
    };
    let name = match name_opt {
        Some(n) => n,
        None => bail!("get requires namespace/name, e.g. 'sunbeam get ory/kratos-abc'"),
    };

    let client = get_client().await?;
    let api: Api<Pod> = Api::namespaced(client.clone(), ns);

    let pod = api
        .get_opt(name)
        .await?
        .ok_or_else(|| SunbeamError::kube(format!("Pod {ns}/{name} not found.")))?;

    let text = match output {
        "json" => serde_json::to_string_pretty(&pod)?,
        _ => serde_yaml::to_string(&pod)?,
    };
    println!("{text}");
    Ok(())
}

/// Restart deployments. Accepts service names, categories, namespaces, or
/// legacy namespace/name syntax. None restarts all non-infra services with
/// deployments.
#[tracing::instrument(skip(logger, target))]
pub async fn cmd_restart(logger: &crate::logger::Logger, target: Option<&str>) -> Result<()> {
    tracing::info!("Restarting services...");

    let reg = get_registry(logger).await?;

    // Collect (namespace, deployment) pairs to restart.
    let pairs: Vec<(String, String)> = match target {
        None => {
            // All non-infra services with deployments
            reg.all()
                .iter()
                .filter(|s| !s.deployments.is_empty())
                .filter(|s| s.category != Category::Infra)
                .flat_map(|s| {
                    s.deployments
                        .iter()
                        .map(move |d| (s.namespace.clone(), d.clone()))
                })
                .collect()
        }
        Some(input) => {
            let resolved = reg.resolve(input);
            if !resolved.is_empty() {
                resolved
                    .iter()
                    .flat_map(|s| {
                        s.deployments
                            .iter()
                            .map(move |d| (s.namespace.clone(), d.clone()))
                    })
                    .collect()
            } else {
                // Fallback: parse as namespace or namespace/name
                let (ns_filter, svc_filter) = parse_target(Some(input))?;
                match (ns_filter, svc_filter) {
                    (Some(ns), None) => {
                        // Restart all deployments in this namespace from registry
                        reg.by_namespace(ns)
                            .iter()
                            .flat_map(|s| {
                                s.deployments
                                    .iter()
                                    .map(move |d| (s.namespace.clone(), d.clone()))
                            })
                            .collect()
                    }
                    (Some(ns), Some(name)) => {
                        vec![(ns.to_string(), name.to_string())]
                    }
                    _ => vec![],
                }
            }
        }
    };

    if pairs.is_empty() {
        tracing::warn!(
            "No matching services for target: {}",
            target.unwrap_or("(none)")
        );
        return Ok(());
    }

    for (ns, dep) in &pairs {
        if let Err(e) = kube_rollout_restart(ns, dep).await {
            tracing::warn!("Failed to restart {ns}/{dep}: {e}");
        }
    }
    tracing::info!("Done.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_for_status() {
        assert_eq!(icon_for_status("Running"), "\u{2713}");
        assert_eq!(icon_for_status("Completed"), "\u{2713}");
        assert_eq!(icon_for_status("Succeeded"), "\u{2713}");
        assert_eq!(icon_for_status("Pending"), "\u{25cb}");
        assert_eq!(icon_for_status("Failed"), "\u{2717}");
        assert_eq!(icon_for_status("Unknown"), "?");
        assert_eq!(icon_for_status("CrashLoopBackOff"), "?");
    }

    #[test]
    fn test_pod_ready_string_format() {
        let ready = "2/3";
        let parts: Vec<&str> = ready.split('/').collect();
        assert_eq!(parts.len(), 2);
        assert_ne!(parts[0], parts[1]);
    }

    #[test]
    fn test_unhealthy_detection_by_ready_ratio() {
        let ready = "1/2";
        let status = "Running";
        let mut unhealthy = !matches!(status, "Running" | "Completed" | "Succeeded");
        if !unhealthy && status == "Running" && ready.contains('/') {
            let parts: Vec<&str> = ready.split('/').collect();
            if parts.len() == 2 && parts[0] != parts[1] {
                unhealthy = true;
            }
        }
        assert!(unhealthy);
    }

    #[test]
    fn test_healthy_detection_by_ready_ratio() {
        let ready = "2/2";
        let status = "Running";
        let mut unhealthy = !matches!(status, "Running" | "Completed" | "Succeeded");
        if !unhealthy && status == "Running" && ready.contains('/') {
            let parts: Vec<&str> = ready.split('/').collect();
            if parts.len() == 2 && parts[0] != parts[1] {
                unhealthy = true;
            }
        }
        assert!(!unhealthy);
    }

    #[test]
    fn test_completed_pods_are_healthy() {
        let status = "Completed";
        let unhealthy = !matches!(status, "Running" | "Completed" | "Succeeded");
        assert!(!unhealthy);
    }

    #[test]
    fn test_pending_pods_are_unhealthy() {
        let status = "Pending";
        let unhealthy = !matches!(status, "Running" | "Completed" | "Succeeded");
        assert!(unhealthy);
    }
}
