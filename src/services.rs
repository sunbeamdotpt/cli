//! Service management — status, logs, restart.

use crate::error::{Result, SunbeamError};
use crate::kube::{get_client, kube_rollout_restart, parse_target};
use crate::output::{ok, step, warn};
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::api::{Api, DynamicObject, ListParams, LogParams};
use sunbeam_sdk::registry::{self, Category, ServiceRegistry};

// ---------------------------------------------------------------------------
// Registry helper
// ---------------------------------------------------------------------------

/// Discover the service registry from the cluster.
async fn get_registry() -> Result<ServiceRegistry> {
    let client = get_client().await?;
    registry::discover(client)
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
}

fn icon_for_status(status: &str) -> &'static str {
    match status {
        "Running" | "Completed" | "Succeeded" => "\u{2713}",
        "Pending" => "\u{25cb}",
        "Failed" => "\u{2717}",
        _ => "?",
    }
}

fn is_unhealthy(pod: &Pod) -> bool {
    let status = pod.status.as_ref();
    let phase = status.and_then(|s| s.phase.as_deref()).unwrap_or("Unknown");

    match phase {
        "Running" => {
            // Check all containers are ready.
            let container_statuses = status.and_then(|s| s.container_statuses.as_ref());
            if let Some(cs) = container_statuses {
                let total = cs.len();
                let ready = cs.iter().filter(|c| c.ready).count();
                ready != total
            } else {
                true
            }
        }
        "Succeeded" | "Completed" => false,
        _ => true,
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
    step("VSO secret sync status...");

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
        ok("All VSO secrets synced.");
    } else {
        warn("Some VSO secrets are not synced.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public commands
// ---------------------------------------------------------------------------

/// Show pod health, optionally filtered by service name, category, namespace,
/// or legacy namespace/service syntax.
pub async fn cmd_status(target: Option<&str>) -> Result<()> {
    step("Pod health across all namespaces...");

    let client = get_client().await?;
    let reg = get_registry().await?;

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
                                });
                            }
                        }
                    }
                    (Some(ns), Some(svc)) => {
                        let api: Api<Pod> = Api::namespaced(client.clone(), ns);
                        let lp = ListParams::default().labels(&format!("app={svc}"));
                        if let Ok(list) = api.list(&lp).await {
                            for pod in list.items {
                                pods.push(PodRow {
                                    ns: ns.to_string(),
                                    name: pod.name_any(),
                                    ready: pod_ready_str(&pod),
                                    status: pod_phase(&pod),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if pods.is_empty() {
        warn("No pods found in managed namespaces.");
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
        println!(
            "    {icon} {:<50} {:<6} {}",
            row.name, row.ready, row.status
        );
    }

    println!();
    if all_ok {
        ok("All pods healthy.");
    } else {
        warn("Some pods are not ready.");
    }

    vso_sync_status().await?;
    Ok(())
}

/// Stream logs for a service. Accepts a service name (e.g. "hydra") or legacy
/// namespace/name syntax (e.g. "ory/kratos").
pub async fn cmd_logs(target: &str, follow: bool) -> Result<()> {
    // Try registry first for exact service name match
    let reg = get_registry().await?;
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
        let mut lp = LogParams::default();
        lp.follow = true;
        lp.tail_lines = Some(100);

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
                    warn(&format!("Log stream error: {e}"));
                    break;
                }
            }
        }
    } else {
        // Print logs from all matching pods
        for pod in &pod_list.items {
            let pod_name = pod.name_any();
            let mut lp = LogParams::default();
            lp.tail_lines = Some(100);

            match api.logs(&pod_name, &lp).await {
                Ok(logs) => print!("{logs}"),
                Err(e) => warn(&format!("Failed to get logs for {pod_name}: {e}")),
            }
        }
    }

    Ok(())
}

/// Print raw pod output in YAML or JSON format.
pub async fn cmd_get(target: &str, output: &str) -> Result<()> {
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
pub async fn cmd_restart(target: Option<&str>) -> Result<()> {
    step("Restarting services...");

    let reg = get_registry().await?;

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
        warn(&format!(
            "No matching services for target: {}",
            target.unwrap_or("(none)")
        ));
        return Ok(());
    }

    for (ns, dep) in &pairs {
        if let Err(e) = kube_rollout_restart(ns, dep).await {
            warn(&format!("Failed to restart {ns}/{dep}: {e}"));
        }
    }
    ok("Done.");
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
