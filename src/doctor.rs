//! `sunbeam doctor` — connectivity diagnostics.
//!
//! Checks kubectl, k8s API, VPN daemon, OpenBao, DNS, and service registry
//! in sequence, reporting pass/fail for each.

use sdk::error::Result;
use sdk::info;
use sdk::k8s_openapi;
use sdk::kube_rs as kube;

/// Cmd doctor.
#[tracing::instrument(skip(logger))]
pub async fn cmd_doctor(logger: &sdk::logger::Logger) -> Result<()> {
    info!(logger, "Running diagnostics...");
    println!();

    let mut failures = 0u32;

    // 1. Kube context
    let context = sdk::kube::context();
    if context.is_empty() {
        info!(logger, "kube context: not set");
        failures += 1;
    } else {
        info!(logger, "kube context: configured", context = context);
    }

    // 3. K8s API reachability
    match sdk::kube::get_client().await {
        Ok(client) => {
            use kube::api::Api;
            let ns: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client.clone());
            match ns.list(&Default::default()).await {
                Ok(list) => {
                    info!(
                        logger,
                        "k8s API: reachable",
                        namespace_count = list.items.len()
                    );
                }
                Err(e) => {
                    info!(logger, "k8s API: connected but list failed", error = e);
                    failures += 1;
                }
            }
        }
        Err(e) => {
            info!(logger, "k8s API: unreachable", error = e);
            failures += 1;
        }
    }

    // 4. VPN daemon
    let vpn_sock = sdk::vpn::env::vpn_state_dir()
        .map(|d| d.join("daemon.sock"))
        .unwrap_or_default();
    if vpn_sock.exists() {
        info!(
            logger,
            "VPN daemon: socket exists",
            path = vpn_sock.display()
        );
    } else {
        info!(logger, "VPN daemon: not running (no socket)");
    }

    // 5. Domain config
    let domain = sdk::config::domain();
    if domain.is_empty() {
        info!(
            logger,
            "domain: not configured -- run `sunbeam config set --domain <domain>`"
        );
        failures += 1;
    } else {
        info!(logger, "domain: configured", domain = domain);
    }

    // 6. OpenBao
    if let Some((pod, unlabeled)) = sdk::kube::find_pod_by_label_or_any(
        "openbao",
        "app.kubernetes.io/name=openbao,component=server",
    )
    .await
    {
        let label_note = if unlabeled { " (unlabeled)" } else { "" };
        match sdk::kube::kube_exec("openbao", &pod, &["bao", "status", "-format=json"], None).await
        {
            Ok((0, out)) => {
                let sealed = serde_json::from_str::<serde_json::Value>(&out)
                    .ok()
                    .and_then(|v| v.get("sealed")?.as_bool())
                    .unwrap_or(true);
                if sealed {
                    info!(logger, &format!("OpenBao{label_note}: sealed"));
                    failures += 1;
                } else {
                    info!(logger, &format!("OpenBao{label_note}: unsealed"));
                }
            }
            _ => {
                info!(logger, &format!("OpenBao{label_note}: status check failed"));
                failures += 1;
            }
        }
    } else {
        info!(logger, "OpenBao: pod not found");
        failures += 1;
    }

    // 8. Service registry
    if let Ok(client) = sdk::kube::get_client().await {
        match crate::registry::discover(logger, &client).await {
            Ok(reg) => {
                let count = reg.all().len();
                info!(
                    logger,
                    "service registry: services discovered",
                    count = count
                );
            }
            Err(e) => {
                info!(logger, "service registry: discovery failed", error = e);
                failures += 1;
            }
        }
    }

    // Summary
    println!();
    if failures == 0 {
        info!(logger, "All checks passed.");
    } else {
        info!(logger, "check(s) failed.", failures = failures);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn doctor_module_compiles() {
        // Smoke test -- actual diagnostics require a cluster.
    }
}
