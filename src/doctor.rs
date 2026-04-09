//! `sunbeam doctor` — connectivity diagnostics.
//!
//! Checks kubectl, k8s API, VPN daemon, OpenBao, DNS, and service registry
//! in sequence, reporting pass/fail for each.

use crate::error::Result;
use crate::output::{ok, step, warn};

pub async fn cmd_doctor() -> Result<()> {
    step("Running diagnostics...");
    println!();

    let mut failures = 0u32;

    // 1. kubectl binary
    match tokio::process::Command::new("kubectl")
        .args(["version", "--client", "--short"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let ver = String::from_utf8_lossy(&out.stdout);
            ok(&format!("kubectl: {}", ver.trim()));
        }
        _ => {
            warn("kubectl: not found or not working");
            failures += 1;
        }
    }

    // 2. Kube context
    let context = crate::kube::context();
    if context.is_empty() {
        warn("kube context: not set");
        failures += 1;
    } else {
        ok(&format!("kube context: {context}"));
    }

    // 3. K8s API reachability
    match crate::kube::get_client().await {
        Ok(client) => {
            use kube::api::Api;
            let ns: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client.clone());
            match ns.list(&Default::default()).await {
                Ok(list) => {
                    ok(&format!("k8s API: reachable ({} namespaces)", list.items.len()));
                }
                Err(e) => {
                    warn(&format!("k8s API: connected but list failed: {e}"));
                    failures += 1;
                }
            }
        }
        Err(e) => {
            warn(&format!("k8s API: unreachable ({e})"));
            failures += 1;
        }
    }

    // 4. VPN daemon
    let vpn_sock = dirs::runtime_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/run")))
        .unwrap_or_default()
        .join("sunbeam-vpn.sock");
    if vpn_sock.exists() {
        ok(&format!("VPN daemon: socket exists at {}", vpn_sock.display()));
    } else {
        warn("VPN daemon: not running (no socket)");
    }

    // 5. Domain config
    let domain = crate::config::domain();
    if domain.is_empty() {
        warn("domain: not configured -- run `sunbeam config set --domain <domain>`");
        failures += 1;
    } else {
        ok(&format!("domain: {domain}"));
    }

    // 6. SSH host (production only)
    let ctx = crate::config::active_context();
    if !ctx.ssh_host.is_empty() {
        ok(&format!("ssh host: {}", ctx.ssh_host));
    }

    // 7. OpenBao
    if let Some(pod) = crate::kube::find_pod_by_label(
        "data",
        "app.kubernetes.io/name=openbao,component=server",
    )
    .await
    {
        match crate::kube::kube_exec("data", &pod, &["bao", "status", "-format=json"], None).await
        {
            Ok((0, out)) => {
                let sealed = serde_json::from_str::<serde_json::Value>(&out)
                    .ok()
                    .and_then(|v| v.get("sealed")?.as_bool())
                    .unwrap_or(true);
                if sealed {
                    warn("OpenBao: sealed");
                    failures += 1;
                } else {
                    ok("OpenBao: unsealed");
                }
            }
            _ => {
                warn("OpenBao: status check failed");
                failures += 1;
            }
        }
    } else {
        warn("OpenBao: pod not found");
        failures += 1;
    }

    // 8. Service registry
    if let Ok(client) = crate::kube::get_client().await {
        match sunbeam_sdk::registry::discover(client).await {
            Ok(reg) => {
                let count = reg.all().len();
                ok(&format!("service registry: {count} service(s) discovered"));
            }
            Err(e) => {
                warn(&format!("service registry: discovery failed ({e})"));
                failures += 1;
            }
        }
    }

    // Summary
    println!();
    if failures == 0 {
        ok("All checks passed.");
    } else {
        warn(&format!("{failures} check(s) failed."));
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
