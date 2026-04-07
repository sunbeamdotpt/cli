//! Service management — status, logs, restart.

use crate::error::{Result, SunbeamError};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DynamicObject, ListParams, LogParams};
use kube::ResourceExt;
use std::collections::BTreeMap;
use crate::constants::MANAGED_NS;
use crate::kube::{get_client, kube_rollout_restart, parse_target};
use crate::output::{ok, step, warn};

/// Services that can be rollout-restarted, as (namespace, deployment) pairs.
pub const SERVICES_TO_RESTART: &[(&str, &str)] = &[
    ("ory", "hydra"),
    ("ory", "kratos"),
    ("ory", "login-ui"),
    ("devtools", "gitea"),
    ("storage", "seaweedfs-filer"),
    ("matrix", "tuwunel"),
    ("media", "livekit-server"),
];

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
    let phase = status
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown");

    match phase {
        "Running" => {
            // Check all containers are ready.
            let container_statuses = status
                .and_then(|s| s.container_statuses.as_ref());
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
            let mut grouped: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
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
            let mut grouped: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
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

/// Show pod health, optionally filtered by namespace or namespace/service.
pub async fn cmd_status(target: Option<&str>) -> Result<()> {
    step("Pod health across all namespaces...");

    let client = get_client().await?;
    let (ns_filter, svc_filter) = parse_target(target)?;

    let mut pods: Vec<PodRow> = Vec::new();

    match (ns_filter, svc_filter) {
        (None, _) => {
            // All managed namespaces
            let ns_set: std::collections::HashSet<&str> =
                MANAGED_NS.iter().copied().collect();
            for ns in MANAGED_NS {
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
        (Some(ns), None) => {
            // All pods in a namespace
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
            // Specific service: filter by app label
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

        let mut unhealthy = !matches!(
            row.status.as_str(),
            "Running" | "Completed" | "Succeeded"
        );
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
        println!("    {icon} {:<50} {:<6} {}", row.name, row.ready, row.status);
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

/// Stream logs for a service. Target must include service name (e.g. ory/kratos).
pub async fn cmd_logs(target: &str, follow: bool) -> Result<()> {
    let (ns_opt, name_opt) = parse_target(Some(target))?;
    let ns = ns_opt.unwrap_or("");
    let name = match name_opt {
        Some(n) => n,
        None => bail!("Logs require a service name, e.g. 'ory/kratos'."),
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

/// Restart deployments. None=all, 'ory'=namespace, 'ory/kratos'=specific.
pub async fn cmd_restart(target: Option<&str>) -> Result<()> {
    step("Restarting services...");

    let (ns_filter, svc_filter) = parse_target(target)?;

    let matched: Vec<(&str, &str)> = match (ns_filter, svc_filter) {
        (None, _) => SERVICES_TO_RESTART.to_vec(),
        (Some(ns), None) => SERVICES_TO_RESTART
            .iter()
            .filter(|(n, _)| *n == ns)
            .copied()
            .collect(),
        (Some(ns), Some(name)) => SERVICES_TO_RESTART
            .iter()
            .filter(|(n, d)| *n == ns && *d == name)
            .copied()
            .collect(),
    };

    if matched.is_empty() {
        warn(&format!(
            "No matching services for target: {}",
            target.unwrap_or("(none)")
        ));
        return Ok(());
    }

    for (ns, dep) in &matched {
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
    fn test_managed_ns_contains_expected() {
        assert!(MANAGED_NS.contains(&"ory"));
        assert!(MANAGED_NS.contains(&"data"));
        assert!(MANAGED_NS.contains(&"devtools"));
        assert!(MANAGED_NS.contains(&"ingress"));
        assert!(MANAGED_NS.contains(&"matrix"));
        assert!(MANAGED_NS.contains(&"media"));
        assert!(MANAGED_NS.contains(&"stalwart"));
        assert!(MANAGED_NS.contains(&"storage"));
        assert!(MANAGED_NS.contains(&"monitoring"));
        assert!(MANAGED_NS.contains(&"vault-secrets-operator"));
        assert!(MANAGED_NS.contains(&"vpn"));
        assert!(MANAGED_NS.contains(&"wfe"));
        assert!(!MANAGED_NS.contains(&"lasuite"));
    }

    #[test]
    fn test_services_to_restart_contains_expected() {
        assert!(SERVICES_TO_RESTART.contains(&("ory", "hydra")));
        assert!(SERVICES_TO_RESTART.contains(&("ory", "kratos")));
        assert!(SERVICES_TO_RESTART.contains(&("ory", "login-ui")));
        assert!(SERVICES_TO_RESTART.contains(&("devtools", "gitea")));
        assert!(SERVICES_TO_RESTART.contains(&("storage", "seaweedfs-filer")));
        assert!(SERVICES_TO_RESTART.contains(&("matrix", "tuwunel")));
        assert!(SERVICES_TO_RESTART.contains(&("media", "livekit-server")));
        assert!(!SERVICES_TO_RESTART.iter().any(|(ns, _)| *ns == "lasuite"));
        assert_eq!(SERVICES_TO_RESTART.len(), 7);
    }

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
    fn test_restart_filter_namespace() {
        let matched: Vec<(&str, &str)> = SERVICES_TO_RESTART
            .iter()
            .filter(|(n, _)| *n == "ory")
            .copied()
            .collect();
        assert_eq!(matched.len(), 3);
        assert!(matched.contains(&("ory", "hydra")));
        assert!(matched.contains(&("ory", "kratos")));
        assert!(matched.contains(&("ory", "login-ui")));
    }

    #[test]
    fn test_restart_filter_specific() {
        let matched: Vec<(&str, &str)> = SERVICES_TO_RESTART
            .iter()
            .filter(|(n, d)| *n == "ory" && *d == "kratos")
            .copied()
            .collect();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0], ("ory", "kratos"));
    }

    #[test]
    fn test_restart_filter_no_match() {
        let matched: Vec<(&str, &str)> = SERVICES_TO_RESTART
            .iter()
            .filter(|(n, d)| *n == "nonexistent" && *d == "nosuch")
            .copied()
            .collect();
        assert!(matched.is_empty());
    }

    #[test]
    fn test_restart_filter_all() {
        let matched: Vec<(&str, &str)> = SERVICES_TO_RESTART.to_vec();
        assert_eq!(matched.len(), 7);
    }

    #[test]
    fn test_pod_ready_string_format() {
        // Verify format: "N/M"
        let ready = "2/3";
        let parts: Vec<&str> = ready.split('/').collect();
        assert_eq!(parts.len(), 2);
        assert_ne!(parts[0], parts[1]); // unhealthy
    }

    #[test]
    fn test_unhealthy_detection_by_ready_ratio() {
        // Simulate the ready ratio check used in cmd_status
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
