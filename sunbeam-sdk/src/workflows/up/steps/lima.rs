//! Lima VM lifecycle steps for local k3s deployments.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::output::{ok, step, warn};
use crate::workflows::data::UpData;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Embedded Lima VM definition for the sunbeam stack.
static LIMA_SUNBEAM_YAML: &str =
    include_str!(concat!(env!("OUT_DIR"), "/lima-sunbeam.yaml"));

// ── EnsureLimaVm ────────────────────────────────────────────────────────────

/// Ensure the Lima `sunbeam` VM exists and is running.
///
/// This step is a no-op for non-local domains (production). For local dev
/// (sslip.io, nip.io, localhost, private IP ranges) it:
///
/// 1. Checks whether `limactl` is installed.
/// 2. Creates the VM from the embedded `lima-sunbeam.yaml` if missing.
/// 3. Starts the VM if it exists but is stopped.
/// 4. Waits for the VM status to become `Running`.
/// 5. Waits for k3s kubeconfig to be available inside the VM.
#[derive(Default)]
pub struct EnsureLimaVm;

#[async_trait::async_trait]
impl StepBody for EnsureLimaVm {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(format!("UpData parse: {e}")))?;

        let domain = if data.domain.is_empty() {
            data.ctx
                .as_ref()
                .map(|c| c.domain.as_str())
                .unwrap_or("")
        } else {
            &data.domain
        };

        if !data.use_lima {
            ok("--use-lima not set — skipping Lima VM management.");
            return Ok(ExecutionResult::next());
        }

        step("Ensuring Lima VM 'sunbeam'...");

        // Verify limactl is available
        let limactl_check = tokio::process::Command::new("limactl")
            .arg("--version")
            .output()
            .await;
        if limactl_check.is_err() {
            return Err(step_err(
                "limactl not found in PATH. Install Lima: https://github.com/lima-vm/lima",
            ));
        }

        let status = lima_vm_status().await;

        match status.as_deref() {
            None | Some("") | Some("None") => {
                step("Creating Lima VM 'sunbeam'...");
                create_lima_vm().await.map_err(step_err)?;
            }
            Some("Running") => {
                ok("Lima VM 'sunbeam' is already running.");
            }
            Some(st) => {
                step(&format!("Lima VM 'sunbeam' is {st} — starting..."));
                start_lima_vm().await.map_err(step_err)?;
            }
        }

        // Wait for VM to report Running
        let vm_deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if std::time::Instant::now() > vm_deadline {
                return Err(step_err("Timed out waiting for Lima VM to start"));
            }
            match lima_vm_status().await.as_deref() {
                Some("Running") => break,
                Some(st) => {
                    step(&format!("Waiting for Lima VM (status: {st})..."));
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                None => {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
        ok("Lima VM 'sunbeam' is running.");

        // Paths for kubeconfig merging (used below).
        let lima_kc = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".lima/sunbeam/copied-from-guest/kubeconfig.yaml");
        let host_kc = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".kube/config");

        // Wait for k3s kubeconfig to be copied out by Lima, then verify the
        // cluster API is reachable using the Rust k8s client (no shelling out).
        step("Waiting for k3s to be ready...");
        let k3s_deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut k3s_ready = false;
        loop {
            if std::time::Instant::now() > k3s_deadline {
                return Err(step_err(
                    "Timed out waiting for k3s API to become reachable",
                ));
            }

            // Lima copies the guest kubeconfig here once k3s is initialised.
            if lima_kc.exists() {
                // Try to create a kube client from the Lima kubeconfig.
                match load_kubeconfig_and_probe(&lima_kc).await {
                    Ok(true) => {
                        k3s_ready = true;
                        break;
                    }
                    Ok(false) => {
                        // kubeconfig exists but API not yet responding
                    }
                    Err(e) => {
                        step(&format!("k3s probe error: {e}"));
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        ok("k3s API is reachable.");

        if lima_kc.exists() {
            step("Updating host kubeconfig from Lima VM...");
            match merge_kubeconfigs(&lima_kc, &host_kc).await {
                Ok(merged_yaml) => {
                    if let Some(parent) = host_kc.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&host_kc, merged_yaml) {
                        warn(&format!("Failed to write host kubeconfig: {e}"));
                    } else {
                        #[cfg(unix)]
                        let _ = std::fs::set_permissions(
                            &host_kc,
                            std::fs::Permissions::from_mode(0o600),
                        );
                        ok("Host kubeconfig updated.");
                    }
                }
                Err(e) => {
                    warn(&format!("Kubeconfig merge failed: {e}. Using Lima kubeconfig directly."));
                    if let Some(parent) = host_kc.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Ok(yaml) = std::fs::read_to_string(&lima_kc) {
                        let _ = std::fs::write(&host_kc, yaml);
                    }
                }
            }
        }

        Ok(ExecutionResult::next())
    }
}

/// Query the status of the `sunbeam` Lima VM.
async fn lima_vm_status() -> Option<String> {
    let output = tokio::process::Command::new("limactl")
        .args(["list", "sunbeam", "--format", "{{.Status}}"])
        .output()
        .await
        .ok()?;
    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if status.is_empty() || status == "None" {
        None
    } else {
        Some(status)
    }
}

/// Create the `sunbeam` Lima VM from the embedded YAML.
async fn create_lima_vm() -> Result<(), String> {
    let tmp = std::env::temp_dir().join("lima-sunbeam.yaml");
    std::fs::write(&tmp, LIMA_SUNBEAM_YAML)
        .map_err(|e| format!("Failed to write temp lima yaml: {e}"))?;

    let status = tokio::process::Command::new("limactl")
        .args(["create", "--name", "sunbeam", "--tty=false"])
        .arg(&tmp)
        .status()
        .await
        .map_err(|e| format!("Failed to run limactl create: {e}"))?;

    if !status.success() {
        return Err(format!("limactl create exited with status: {status}"));
    }

    // Start the VM after creation
    start_lima_vm().await
}

/// Start the `sunbeam` Lima VM.
async fn start_lima_vm() -> Result<(), String> {
    let status = tokio::process::Command::new("limactl")
        .args(["start", "sunbeam"])
        .status()
        .await
        .map_err(|e| format!("Failed to run limactl start: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("limactl start exited with status: {status}"))
    }
}

/// Load a kubeconfig file and attempt to list nodes to verify the cluster
/// API is reachable.
async fn load_kubeconfig_and_probe(path: &std::path::Path) -> Result<bool, String> {
    use kube::config::{Config, KubeConfigOptions, Kubeconfig};
    use kube::Client;

    let kc = Kubeconfig::read_from(path)
        .map_err(|e| format!("Failed to read kubeconfig from {}: {e}", path.display()))?;

    let opts = KubeConfigOptions::default();
    let config = Config::from_custom_kubeconfig(kc, &opts)
        .await
        .map_err(|e| format!("Failed to build config: {e}"))?;

    let client = Client::try_from(config)
        .map_err(|e| format!("Failed to create client: {e}"))?;

    let nodes: kube::Api<k8s_openapi::api::core::v1::Node> = kube::Api::all(client);
    match nodes.list(&kube::api::ListParams::default()).await {
        Ok(list) => Ok(!list.items.is_empty()),
        Err(_) => Ok(false),
    }
}

/// Merge two kubeconfigs in pure Rust.
///
/// Concatenates clusters, auth_infos (users), and contexts from both files.
/// Preserves the host file's current-context unless it's empty.
async fn merge_kubeconfigs(
    lima_path: &std::path::Path,
    host_path: &std::path::Path,
) -> Result<String, String> {
    use kube::config::Kubeconfig;

    let lima_kc = Kubeconfig::read_from(lima_path)
        .map_err(|e| format!("Failed to read Lima kubeconfig: {e}"))?;

    let mut merged = if host_path.exists() {
        Kubeconfig::read_from(host_path)
            .map_err(|e| format!("Failed to read host kubeconfig: {e}"))?
    } else {
        Kubeconfig::default()
    };

    // Append Lima entries, avoiding duplicates by name.
    for cluster in lima_kc.clusters {
        let name = cluster.name.clone();
        if !merged.clusters.iter().any(|c| c.name == name) {
            merged.clusters.push(cluster);
        }
    }

    for auth in lima_kc.auth_infos {
        let name = auth.name.clone();
        if !merged.auth_infos.iter().any(|a| a.name == name) {
            merged.auth_infos.push(auth);
        }
    }

    for ctx in lima_kc.contexts {
        let name = ctx.name.clone();
        if !merged.contexts.iter().any(|c| c.name == name) {
            merged.contexts.push(ctx);
        }
    }

    // Prefer Lima's current-context if the host doesn't have one.
    if merged.current_context.is_none() && lima_kc.current_context.is_some() {
        merged.current_context = lima_kc.current_context;
    }

    serde_yaml::to_string(&merged)
        .map_err(|e| format!("Failed to serialize merged kubeconfig: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_lima_vm_is_default() {
        let _ = EnsureLimaVm;
    }

    #[test]
    fn test_lima_yaml_embedded() {
        assert!(!LIMA_SUNBEAM_YAML.is_empty());
        assert!(LIMA_SUNBEAM_YAML.contains("k3s"));
    }
}
