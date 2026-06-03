//! Lima VM lifecycle steps for local k3s deployments.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::{debug, info, trace};
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
pub struct EnsureLimaVm {
    logger: crate::logger::Logger,
}

impl Default for EnsureLimaVm {
    fn default() -> Self {
        Self {
            logger: crate::logger::Logger::new(crate::logger::TracingSink),
        }
    }
}

#[async_trait::async_trait]
impl StepBody for EnsureLimaVm {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let logger = &self.logger;
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

        let profile = ctx
            .workflow
            .data
            .get("profile")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if profile != "lima" {
            info!(logger, "Profile is not 'lima' — skipping Lima VM management.");
            return Ok(ExecutionResult::next());
        }

        info!(logger, "Ensuring Lima VM '{}'...", vm = crate::constants::LIMA_VM_NAME);

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
                info!(logger, "Creating Lima VM '{}'...", vm = crate::constants::LIMA_VM_NAME);
                create_lima_vm().await.map_err(step_err)?;
            }
            Some("Running") => {
                info!(logger, "Lima VM '{}' is already running.", vm = crate::constants::LIMA_VM_NAME);
            }
            Some(st) => {
                info!(logger, "Lima VM '{}' is stopped — starting... (status: {})", vm = crate::constants::LIMA_VM_NAME, status = st);
                start_lima_vm().await.map_err(step_err)?;
            }
        }

        // Wait for VM to report Running
        let vm_deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut attempt = 0;
        loop {
            attempt += 1;
            if std::time::Instant::now() > vm_deadline {
                return Err(step_err(format!("Timed out waiting for Lima VM '{}' to reach Running status (5 min). Try: limactl list {}", crate::constants::LIMA_VM_NAME, crate::constants::LIMA_VM_NAME)));
            }
            match lima_vm_status().await.as_deref() {
                Some("Running") => {
                    info!(logger, "Lima VM '{}' is running. (attempt: {})", vm = crate::constants::LIMA_VM_NAME, attempt = attempt);
                    break;
                }
                Some(st) => {
                    if attempt % 6 == 0 {
                        info!(
                            logger,
                            "Still waiting for Lima VM...",
                            status = st,
                            attempt = attempt,
                            elapsed_secs = attempt * 5,
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                None => {
                    if attempt % 6 == 0 {
                        info!(
                            logger,
                            "Still waiting for Lima VM (no status yet)...",
                            attempt = attempt,
                            elapsed_secs = attempt * 5,
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }

        // Paths for kubeconfig merging (used below).
        let lima_kc = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(format!(".lima/{}/copied-from-guest/kubeconfig.yaml", crate::constants::LIMA_VM_NAME));
        let host_kc = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".kube/config");

        // Wait for k3s kubeconfig to be copied out by Lima, then verify the
        // cluster API is reachable using the Rust k8s client (no shelling out).
        info!(logger, "Waiting for k3s API to be reachable...");
        let k3s_deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut k3s_ready = false;
        let mut k3s_attempt = 0;
        loop {
            k3s_attempt += 1;
            if std::time::Instant::now() > k3s_deadline {
                return Err(step_err(format!(
                    "Timed out waiting for k3s API to become reachable (5 min).\n\
                     Try: limactl shell {} -- sudo systemctl status k3s",
                    crate::constants::LIMA_VM_NAME
                )));
            }

            // Lima copies the guest kubeconfig here once k3s is initialised.
            if lima_kc.exists() {
                // Try to create a kube client from the Lima kubeconfig.
                match load_kubeconfig_and_probe(&lima_kc).await {
                    Ok(true) => {
                        k3s_ready = true;
                        info!(
                            logger,
                            "k3s API is reachable.",
                            attempt = k3s_attempt,
                        );
                        break;
                    }
                    Ok(false) => {
                        // kubeconfig exists but API not yet responding
                        if k3s_attempt % 6 == 0 {
                            info!(
                                logger,
                                "k3s kubeconfig present but API not responding yet...",
                                attempt = k3s_attempt,
                                elapsed_secs = k3s_attempt * 5,
                            );
                        }
                    }
                    Err(e) => {
                        info!(
                            logger,
                            "k3s probe error (retrying)...",
                            attempt = k3s_attempt,
                            err = e,
                        );
                    }
                }
            } else if k3s_attempt % 6 == 0 {
                info!(
                    logger,
                    "Waiting for Lima to copy k3s kubeconfig...",
                    attempt = k3s_attempt,
                    elapsed_secs = k3s_attempt * 5,
                    path = lima_kc.display(),
                );
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        if lima_kc.exists() {
            info!(logger, "Updating host kubeconfig from Lima VM...");
            match merge_kubeconfigs(&lima_kc, &host_kc).await {
                Ok(merged_yaml) => {
                    if let Some(parent) = host_kc.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&host_kc, merged_yaml) {
                        info!(
                            logger,
                            "Failed to write host kubeconfig.",
                            path = host_kc.display(),
                            err = e,
                        );
                    } else {
                        #[cfg(unix)]
                        let _ = std::fs::set_permissions(
                            &host_kc,
                            std::fs::Permissions::from_mode(0o600),
                        );
                        info!(logger, "Host kubeconfig updated.", path = host_kc.display());
                    }
                }
                Err(e) => {
                    info!(
                        logger,
                        "Kubeconfig merge failed — using Lima kubeconfig directly.",
                        err = e,
                    );
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

/// Query the status of the Lima VM.
async fn lima_vm_status() -> Option<String> {
    let output = tokio::process::Command::new("limactl")
        .args(["list", crate::constants::LIMA_VM_NAME, "--format", "{{.Status}}"])
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

/// Create the Lima VM from the embedded YAML.
async fn create_lima_vm() -> Result<(), String> {
    let tmp = std::env::temp_dir().join("lima-sunbeam.yaml");
    std::fs::write(&tmp, LIMA_SUNBEAM_YAML)
        .map_err(|e| format!("Failed to write temp lima yaml: {e}"))?;

    let status = tokio::process::Command::new("limactl")
        .args(["create", "--name", crate::constants::LIMA_VM_NAME, "--tty=false"])
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

/// Start the Lima VM.
async fn start_lima_vm() -> Result<(), String> {
    let status = tokio::process::Command::new("limactl")
        .args(["start", crate::constants::LIMA_VM_NAME])
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
        Err(e) => Err(format!("k3s API node list failed: {e}")),
    }
}

/// Merge two kubeconfigs in pure Rust.
///
/// Lima entries are renamed to `lima-sunbeam` so they never clash with the
/// user's existing `default` (or any other) context/cluster/user, and it's
/// obvious which context belongs to the Sunbeam Lima VM.
async fn merge_kubeconfigs(
    lima_path: &std::path::Path,
    host_path: &std::path::Path,
) -> Result<String, String> {
    use kube::config::Kubeconfig;

    let mut lima_kc = Kubeconfig::read_from(lima_path)
        .map_err(|e| format!("Failed to read Lima kubeconfig: {e}"))?;

    let mut merged = if host_path.exists() {
        Kubeconfig::read_from(host_path)
            .map_err(|e| format!("Failed to read host kubeconfig: {e}"))?
    } else {
        Kubeconfig::default()
    };

    // Rename Lima clusters, auth_infos, and contexts so they never clash
    // with the user's existing contexts.
    let ctx_name = crate::constants::LIMA_KUBE_CONTEXT;

    for cluster in &mut lima_kc.clusters {
        cluster.name = ctx_name.to_string();
    }

    for auth in &mut lima_kc.auth_infos {
        auth.name = ctx_name.to_string();
    }

    for ctx in &mut lima_kc.contexts {
        if let Some(ref mut c) = ctx.context {
            c.cluster = ctx_name.to_string();
            c.user = Some(ctx_name.to_string());
        }
        ctx.name = ctx_name.to_string();
    }

    // Merge Lima entries, replacing any existing `lima-sunbeam` host entries
    // so recreated VMs (new certificates) always take precedence.
    for cluster in lima_kc.clusters {
        let pos = merged.clusters.iter().position(|c| c.name == cluster.name);
        match pos {
            Some(i) => merged.clusters[i] = cluster,
            None => merged.clusters.push(cluster),
        }
    }

    for auth in lima_kc.auth_infos {
        let pos = merged.auth_infos.iter().position(|a| a.name == auth.name);
        match pos {
            Some(i) => merged.auth_infos[i] = auth,
            None => merged.auth_infos.push(auth),
        }
    }

    for ctx in lima_kc.contexts {
        let pos = merged.contexts.iter().position(|c| c.name == ctx.name);
        match pos {
            Some(i) => merged.contexts[i] = ctx,
            None => merged.contexts.push(ctx),
        }
    }

    // Always set current-context to the Lima context after an up run so
    // kubectl works out of the box.
    merged.current_context = Some(crate::constants::LIMA_KUBE_CONTEXT.to_string());

    serde_yaml::to_string(&merged)
        .map_err(|e| format!("Failed to serialize merged kubeconfig: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_lima_vm_is_default() {
        let _ = EnsureLimaVm {
            logger: crate::logger::Logger::new(crate::logger::NoopSink),
        };
    }

    #[test]
    fn test_lima_yaml_embedded() {
        assert!(!LIMA_SUNBEAM_YAML.is_empty());
        assert!(LIMA_SUNBEAM_YAML.contains("k3s"));
    }
}
