//! Bootstrap critical infrastructure images before the ingress controller is ready.
//!
//! On a fresh install the proxy image doesn't exist in any registry, but the
//! ingress controller (pingora) needs it to start. This step builds the proxy
//! image using the host Docker daemon (the one allowed shell-out), saves it as
//! a tarball, and imports it into k3s containerd via a temporary pod that
//! mounts the host containerd socket — no `kubectl` subprocess required.
//!
//! This breaks the chicken-and-egg cycle: proxy image → ingress → registries.

use std::path::PathBuf;
use std::time::Duration;

use k8s_openapi::api::core::v1::{Container, HostPathVolumeSource, Pod, PodSpec, Volume, VolumeMount};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};


use crate::workflows::data::UpData;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Build critical infrastructure images and import into k3s containerd.
#[derive(Default)]
pub struct BootstrapCriticalImages;

#[async_trait::async_trait]
impl StepBody for BootstrapCriticalImages {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let lima_skip: Vec<String> = ctx
            .workflow
            .data
            .get("lima_skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if lima_skip.contains(&"ingress".to_string()) {
            tracing::info!("Skipping proxy image bootstrap (Lima VM — ingress skipped)");
            return Ok(ExecutionResult::next());
        }

        let data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| step_err(e.to_string()))?;

        let step_ctx = data
            .ctx
            .as_ref()
            .ok_or_else(|| step_err("missing __ctx"))?;

        let domain = if data.domain.is_empty() {
            &step_ctx.domain
        } else {
            &data.domain
        };

        tracing::info!("Bootstrapping critical images...");

        // 1. Check if proxy image already exists in k3s
        let proxy_tag = "579e975983";
        let proxy_image = format!("oci.{domain}/studio/proxy:{proxy_tag}");
        if image_exists_in_k3s(&proxy_image).await? {
            tracing::info!("Proxy image already present in k3s.");
            return Ok(ExecutionResult::next());
        }

        // 2. Build proxy image using host Docker
        tracing::info!("Building proxy image...");
        let tar_path = build_proxy_image().await?;

        // 3. Import into k3s containerd
        tracing::info!("Importing proxy image into k3s...");
        import_image_into_k3s(&tar_path, &proxy_image).await?;

        tracing::info!("Proxy image bootstrapped.");
        Ok(ExecutionResult::next())
    }
}

/// Create a kube::Client from the active context.
async fn k8s_client() -> wfe_core::Result<kube::Client> {
    crate::kube::get_client()
        .await
        .map_err(|e| step_err(format!("Failed to create k8s client: {e}")))
}

/// Discover the sole node name (local dev assumption: one node).
async fn get_node_name() -> wfe_core::Result<String> {
    let client = k8s_client().await?;
    let nodes: Api<k8s_openapi::api::core::v1::Node> = Api::all(client);
    let list = nodes
        .list(&ListParams::default())
        .await
        .map_err(|e| step_err(format!("Failed to list nodes: {e}")))?;

    let first = list
        .items
        .into_iter()
        .next()
        .ok_or_else(|| step_err("No nodes found in cluster"))?;

    Ok(first.metadata.name.unwrap_or_default())
}

/// Create an ephemeral pod on the target node that mounts the host's
/// containerd socket and `ctr` binary so we can run containerd commands
/// without `kubectl exec` shell-outs.
async fn spawn_ctr_pod(node_name: &str, pod_name: &str) -> wfe_core::Result<()> {
    let client = k8s_client().await?;
    let pods: Api<Pod> = Api::namespaced(client, "default");

    let pod = Pod {
        metadata: kube::api::ObjectMeta {
            name: Some(pod_name.to_string()),
            ..Default::default()
        },
        spec: Some(PodSpec {
            node_name: Some(node_name.to_string()),
            host_pid: Some(true),
            restart_policy: Some("Never".to_string()),
            containers: vec![Container {
                name: "ctr".to_string(),
                image: Some("busybox:1.36".to_string()),
                command: Some(vec!["sleep".to_string(), "3600".to_string()]),
                volume_mounts: Some(vec![
                    VolumeMount {
                        name: "containerd-sock".to_string(),
                        mount_path: "/run/k3s/containerd/containerd.sock".to_string(),
                        ..Default::default()
                    },
                    VolumeMount {
                        name: "ctr-bin".to_string(),
                        mount_path: "/usr/local/bin/ctr".to_string(),
                        ..Default::default()
                    },
                    VolumeMount {
                        name: "host-tmp".to_string(),
                        mount_path: "/host-tmp".to_string(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }],
            volumes: Some(vec![
                Volume {
                    name: "containerd-sock".to_string(),
                    host_path: Some(HostPathVolumeSource {
                        path: "/run/k3s/containerd/containerd.sock".to_string(),
                        type_: Some("Socket".to_string()),
                    }),
                    ..Default::default()
                },
                Volume {
                    name: "ctr-bin".to_string(),
                    host_path: Some(HostPathVolumeSource {
                        path: "/usr/local/bin/ctr".to_string(),
                        type_: Some("File".to_string()),
                    }),
                    ..Default::default()
                },
                Volume {
                    name: "host-tmp".to_string(),
                    host_path: Some(HostPathVolumeSource {
                        path: "/tmp".to_string(),
                        type_: Some("Directory".to_string()),
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };

    pods.create(&PostParams::default(), &pod)
        .await
        .map_err(|e| step_err(format!("Failed to create ctr pod: {e}")))?;

    // Wait for Running (up to 60s).
    for attempt in 0..60 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match pods.get(pod_name).await {
            Ok(p) => {
                if let Some(status) = p.status
                    && let Some(phase) = status.phase
                {
                    if phase == "Running" {
                        tracing::info!(
                            msg = "ctr pod is running.",
                            pod = %pod_name,
                            attempt = attempt + 1,
                        );
                        return Ok(());
                    }
                    if phase == "Failed" || phase == "Error" {
                        let _ = pods.delete(pod_name, &DeleteParams::default()).await;
                        return Err(step_err(format!(
                            "Ctr pod {pod_name} entered {phase} state after {attempt} attempts"
                        )));
                    }
                }
            }
            Err(e) => {
                if attempt % 10 == 0 {
                    tracing::info!(
                        msg = "Waiting for ctr pod to appear...",
                        pod = %pod_name,
                        attempt = attempt + 1,
                        err = %e,
                    );
                }
            }
        }
    }

    let _ = pods.delete(pod_name, &DeleteParams::default()).await;
    Err(step_err(format!(
        "Timed out waiting for ctr pod {pod_name} to start (60s)"
    )))
}

/// Delete the ephemeral ctr pod (best-effort).
async fn delete_ctr_pod(pod_name: &str) {
    if let Ok(client) = k8s_client().await {
        let pods: Api<Pod> = Api::namespaced(client, "default");
        let _ = pods.delete(pod_name, &DeleteParams::default()).await;
    }
}

/// Check if an image reference already exists in k3s containerd.
async fn image_exists_in_k3s(image_ref: &str) -> wfe_core::Result<bool> {
    let node = get_node_name().await?;
    let pod_name = "sunbeam-ctr-check";

    // Spawn a temporary pod on the node.
    if let Err(e) = spawn_ctr_pod(&node, pod_name).await {
        // If pod creation fails, assume image doesn't exist.
        tracing::warn!("Could not spawn ctr pod: {e}");
        return Ok(false);
    }

    let client = match k8s_client().await {
        Ok(c) => c,
        Err(_) => {
            delete_ctr_pod(pod_name).await;
            return Ok(false);
        }
    };

    let result = crate::kube::kube_exec(
        "default",
        pod_name,
        &["/usr/local/bin/ctr", "-n", "k8s.io", "images", "list", "-q"],
        None,
    )
    .await;

    delete_ctr_pod(pod_name).await;

    match result {
        Ok((0, stdout)) => Ok(stdout.lines().any(|line| line.contains(image_ref))),
        _ => Ok(false),
    }
}

/// BuildKit endpoint for the Lima VM's host-level BuildKit daemon.
const BUILDKIT_ENDPOINT: &str = "tcp://127.0.0.1:1234";

async fn build_proxy_image() -> wfe_core::Result<PathBuf> {
    let ws_root = std::env::current_dir()
        .map_err(|e| step_err(format!("Failed to get cwd: {e}")))?;

    let tar_path = std::env::temp_dir().join("sunbeam-proxy-bootstrap.tar");

    // Try buildctl first (works without Docker daemon).
    let buildctl_result = tokio::process::Command::new("buildctl")
        .args([
            "--addr",
            BUILDKIT_ENDPOINT,
            "build",
            "--frontend",
            "dockerfile.v0",
            "--local",
            "context=.",
            "--local",
            "dockerfile=platform/proxy",
            "--opt",
            "filename=Dockerfile",
            "--output",
            &format!("type=docker,name=sunbeam-proxy:bootstrap,dest={}", tar_path.display()),
        ])
        .current_dir(&ws_root)
        .output()
        .await;

    match buildctl_result {
        Ok(output) if output.status.success() => return Ok(tar_path),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("buildctl failed, falling back to docker buildx: {stderr}");
        }
        Err(e) => {
            tracing::warn!("buildctl not available, falling back to docker buildx: {e}");
        }
    }

    // Fallback: docker buildx with remote BuildKit builder.
    let _ = tokio::process::Command::new("docker")
        .args([
            "buildx",
            "create",
            "--name",
            "sunbeam-buildkit",
            "--driver",
            "remote",
            BUILDKIT_ENDPOINT,
        ])
        .env("DOCKER_HOST", "")
        .output()
        .await;

    let build_output = tokio::process::Command::new("docker")
        .args([
            "buildx",
            "build",
            "--builder",
            "sunbeam-buildkit",
            "-f",
            "platform/proxy/Dockerfile",
            "-t",
            "sunbeam-proxy:bootstrap",
            "--output",
            &format!("type=docker,dest={}", tar_path.display()),
            ".",
        ])
        .env("DOCKER_HOST", "")
        .current_dir(&ws_root)
        .output()
        .await
        .map_err(|e| step_err(format!("Failed to run docker buildx: {e}")))?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        return Err(step_err(format!("docker buildx failed: {stderr}")));
    }

    Ok(tar_path)
}

async fn import_image_into_k3s(tar_path: &PathBuf, target_ref: &str) -> wfe_core::Result<()> {
    let node = get_node_name().await?;
    let pod_name = "sunbeam-ctr-import";
    let host_tar = "/host-tmp/sunbeam-proxy-bootstrap.tar";

    // Copy the tar directly to the Lima VM's /tmp via limactl copy.
    // The ctr pod mounts the node's /tmp as /host-tmp, so it can read
    // the file without streaming it through the exec websocket.
    tracing::info!("Copying tar to Lima VM...");
    let copy_output = tokio::process::Command::new("limactl")
        .args([
            "copy",
            &tar_path.display().to_string(),
            "sunbeam:/tmp/sunbeam-proxy-bootstrap.tar",
        ])
        .output()
        .await
        .map_err(|e| step_err(format!("Failed to run limactl copy: {e}")))?;

    if !copy_output.status.success() {
        let stderr = String::from_utf8_lossy(&copy_output.stderr);
        return Err(step_err(format!("limactl copy failed: {stderr}")));
    }

    spawn_ctr_pod(&node, pod_name).await?;

    // Import the image.
    let import_result = crate::kube::kube_exec(
        "default",
        pod_name,
        &["/usr/local/bin/ctr", "-n", "k8s.io", "images", "import", host_tar],
        None,
    )
    .await;

    if let Err(e) = import_result {
        delete_ctr_pod(pod_name).await;
        return Err(step_err(format!("ctr images import failed: {e}")));
    }
    if let Ok((code, stderr)) = import_result {
        if code != 0 {
            delete_ctr_pod(pod_name).await;
            return Err(step_err(format!(
                "ctr images import failed: {stderr}"
            )));
        }
    }

    // Find the imported image name and tag it.
    let list_result = crate::kube::kube_exec(
        "default",
        pod_name,
        &["/usr/local/bin/ctr", "-n", "k8s.io", "images", "list", "-q"],
        None,
    )
    .await;

    let imported_name = match list_result {
        Ok((0, stdout)) => stdout
            .lines()
            .find(|line| line.contains("sunbeam-proxy"))
            .map(|s| s.trim().to_string()),
        _ => None,
    };

    let imported_name = match imported_name {
        Some(name) => name,
        None => {
            delete_ctr_pod(pod_name).await;
            return Err(step_err(
                "Image imported but 'sunbeam-proxy' not found in ctr images list."
                    .to_string(),
            ));
        }
    };

    let tag_result = crate::kube::kube_exec(
        "default",
        pod_name,
        &[
            "/usr/local/bin/ctr",
            "-n",
            "k8s.io",
            "images",
            "tag",
            &imported_name,
            target_ref,
        ],
        None,
    )
    .await;

    delete_ctr_pod(pod_name).await;

    match tag_result {
        Ok((0, _)) => Ok(()),
        Ok((code, stderr)) => Err(step_err(format!(
            "ctr images tag failed (code {code}): {stderr}"
        ))),
        Err(e) => Err(step_err(format!("ctr images tag failed: {e}"))),
    }
}
