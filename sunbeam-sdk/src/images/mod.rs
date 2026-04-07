//! Image building, mirroring, and pushing to Gitea registry.

pub mod builders;

use crate::error::{Result, ResultExt, SunbeamError};
use base64::Engine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::constants::{GITEA_ADMIN_USER, MANAGED_NS};
use crate::output::{ok, step, warn};

// ---------------------------------------------------------------------------
// BuildTarget enum (moved from cli.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum BuildTarget {
    Proxy,
    KratosAdmin,
    Tuwunel,
    Sol,
}

impl std::fmt::Display for BuildTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BuildTarget::Proxy => "proxy",
            BuildTarget::KratosAdmin => "kratos-admin",
            BuildTarget::Tuwunel => "tuwunel",
            BuildTarget::Sol => "sol",
        };
        write!(f, "{s}")
    }
}

/// amd64-only images that need mirroring: (source, org, repo, tag).
const AMD64_ONLY_IMAGES: &[(&str, &str, &str, &str)] = &[];

// ---------------------------------------------------------------------------
// Build environment
// ---------------------------------------------------------------------------

/// Resolved build environment — production (remote k8s) or local.
#[derive(Debug, Clone)]
pub struct BuildEnv {
    pub is_prod: bool,
    pub domain: String,
    pub registry: String,
    pub admin_pass: String,
    pub platform: String,
    pub ssh_host: Option<String>,
}

/// Detect prod vs local and resolve registry credentials.
pub(crate) async fn get_build_env() -> Result<BuildEnv> {
    let ssh = crate::kube::ssh_host();
    let is_prod = !ssh.is_empty();

    let domain = crate::kube::get_domain().await?;

    // Fetch gitea admin password from the cluster secret
    let admin_pass = crate::kube::kube_get_secret_field(
        "devtools",
        "gitea-admin-credentials",
        "password",
    )
    .await
    .ctx("gitea-admin-credentials secret not found -- run seed first.")?;

    let platform = if is_prod {
        "linux/amd64".to_string()
    } else {
        "linux/arm64".to_string()
    };

    let ssh_host = if is_prod {
        Some(ssh.to_string())
    } else {
        None
    };

    Ok(BuildEnv {
        is_prod,
        domain: domain.clone(),
        registry: format!("src.{domain}"),
        admin_pass,
        platform,
        ssh_host,
    })
}

// ---------------------------------------------------------------------------
// buildctl build + push
// ---------------------------------------------------------------------------

/// Build and push an image via buildkitd running in k8s.
///
/// Port-forwards to the buildkitd service in the `build` namespace,
/// runs `buildctl build`, and pushes the image directly to the Gitea
/// registry from inside the cluster.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn buildctl_build_and_push(
    env: &BuildEnv,
    image: &str,
    dockerfile: &Path,
    context_dir: &Path,
    target: Option<&str>,
    build_args: Option<&HashMap<String, String>>,
    _no_cache: bool,
) -> Result<()> {
    // Find a free local port for port-forward
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .ctx("Failed to bind ephemeral port")?;
    let local_port = listener.local_addr()?.port();
    drop(listener);

    // Build docker config for registry auth
    let auth_token = base64::engine::general_purpose::STANDARD
        .encode(format!("{GITEA_ADMIN_USER}:{}", env.admin_pass));
    let docker_cfg = serde_json::json!({
        "auths": {
            &env.registry: { "auth": auth_token }
        }
    });

    let tmpdir = tempfile::TempDir::new().ctx("Failed to create temp dir")?;
    let cfg_path = tmpdir.path().join("config.json");
    std::fs::write(&cfg_path, serde_json::to_string(&docker_cfg)?)
        .ctx("Failed to write docker config")?;

    // Start port-forward to buildkitd
    let ctx_arg = format!("--context={}", crate::kube::context());
    let pf_port_arg = format!("{local_port}:1234");

    let mut pf = tokio::process::Command::new("kubectl")
        .args([
            &ctx_arg,
            "port-forward",
            "-n",
            "build",
            "svc/buildkitd",
            &pf_port_arg,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ctx("Failed to start buildkitd port-forward")?;

    // Wait for port-forward to become ready
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if tokio::time::Instant::now() > deadline {
            pf.kill().await.ok();
            return Err(SunbeamError::tool("buildctl", format!("buildkitd port-forward on :{local_port} did not become ready within 15s")));
        }
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{local_port}"))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // Build the buildctl command
    let dockerfile_parent = dockerfile
        .parent()
        .unwrap_or(dockerfile)
        .to_string_lossy()
        .to_string();
    let dockerfile_name = dockerfile
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let context_str = context_dir.to_string_lossy().to_string();

    let mut cmd_args = vec![
        "build".to_string(),
        "--frontend".to_string(),
        "dockerfile.v0".to_string(),
        "--local".to_string(),
        format!("context={context_str}"),
        "--local".to_string(),
        format!("dockerfile={dockerfile_parent}"),
        "--opt".to_string(),
        format!("filename={dockerfile_name}"),
        "--opt".to_string(),
        format!("platform={}", env.platform),
        "--output".to_string(),
        format!("type=image,name={image},push=true"),
    ];

    if let Some(tgt) = target {
        cmd_args.push("--opt".to_string());
        cmd_args.push(format!("target={tgt}"));
    }

    if _no_cache {
        cmd_args.push("--no-cache".to_string());
    }

    if let Some(args) = build_args {
        for (k, v) in args {
            cmd_args.push("--opt".to_string());
            cmd_args.push(format!("build-arg:{k}={v}"));
        }
    }

    let buildctl_host = format!("tcp://127.0.0.1:{local_port}");
    let tmpdir_str = tmpdir.path().to_string_lossy().to_string();

    let result = tokio::process::Command::new("buildctl")
        .args(&cmd_args)
        .env("BUILDKIT_HOST", &buildctl_host)
        .env("DOCKER_CONFIG", &tmpdir_str)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await;

    // Always terminate port-forward
    pf.kill().await.ok();
    pf.wait().await.ok();

    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => return Err(SunbeamError::tool("buildctl", format!("exited with status {status}"))),
        Err(e) => return Err(SunbeamError::tool("buildctl", format!("failed to run: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// build_image wrapper
// ---------------------------------------------------------------------------

/// Build a container image via buildkitd and push to the Gitea registry.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_image(
    env: &BuildEnv,
    image: &str,
    dockerfile: &Path,
    context_dir: &Path,
    target: Option<&str>,
    build_args: Option<&HashMap<String, String>>,
    push: bool,
    no_cache: bool,
    cleanup_paths: &[PathBuf],
) -> Result<()> {
    ok(&format!(
        "Building image ({}{})...",
        env.platform,
        target
            .map(|t| format!(", {t} target"))
            .unwrap_or_default()
    ));

    if !push {
        warn("Builds require --push (buildkitd pushes directly to registry); skipping.");
        return Ok(());
    }

    let result = buildctl_build_and_push(
        env,
        image,
        dockerfile,
        context_dir,
        target,
        build_args,
        no_cache,
    )
    .await;

    // Cleanup
    for p in cleanup_paths {
        if p.exists() {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Node operations
// ---------------------------------------------------------------------------

/// Return one SSH-reachable IP per node in the cluster.
async fn get_node_addresses() -> Result<Vec<String>> {
    let client = crate::kube::get_client().await?;
    let api: kube::api::Api<k8s_openapi::api::core::v1::Node> =
        kube::api::Api::all(client.clone());

    let node_list = api
        .list(&kube::api::ListParams::default())
        .await
        .ctx("Failed to list nodes")?;

    let mut addresses = Vec::new();
    for node in &node_list.items {
        if let Some(status) = &node.status {
            if let Some(addrs) = &status.addresses {
                // Prefer IPv4 InternalIP
                let mut ipv4: Option<String> = None;
                let mut any_internal: Option<String> = None;

                for addr in addrs {
                    if addr.type_ == "InternalIP" {
                        if !addr.address.contains(':') {
                            ipv4 = Some(addr.address.clone());
                        } else if any_internal.is_none() {
                            any_internal = Some(addr.address.clone());
                        }
                    }
                }

                if let Some(ip) = ipv4.or(any_internal) {
                    addresses.push(ip);
                }
            }
        }
    }

    Ok(addresses)
}

/// SSH to each k3s node and pull images into containerd.
pub(crate) async fn ctr_pull_on_nodes(env: &BuildEnv, images: &[String]) -> Result<()> {
    if images.is_empty() {
        return Ok(());
    }

    let nodes = get_node_addresses().await?;
    if nodes.is_empty() {
        warn("Could not detect node addresses; skipping ctr pull.");
        return Ok(());
    }

    let ssh_user = env
        .ssh_host
        .as_deref()
        .and_then(|h| h.split('@').next())
        .unwrap_or("root");

    for node_ip in &nodes {
        for img in images {
            ok(&format!("Pulling {img} into containerd on {node_ip}..."));
            let status = tokio::process::Command::new("ssh")
                .args([
                    "-p",
                    "2222",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &format!("{ssh_user}@{node_ip}"),
                    &format!("sudo ctr -n k8s.io images pull {img}"),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .status()
                .await;

            match status {
                Ok(s) if s.success() => ok(&format!("Pulled {img} on {node_ip}")),
                _ => return Err(SunbeamError::tool("ctr", format!("pull failed on {node_ip} for {img}"))),
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Deploy rollout
// ---------------------------------------------------------------------------

/// Apply manifests for the target namespace and rolling-restart the given deployments.
pub(crate) async fn deploy_rollout(
    env: &BuildEnv,
    deployments: &[&str],
    namespace: &str,
    timeout_secs: u64,
    images: Option<&[String]>,
) -> Result<()> {
    let env_str = if env.is_prod { "production" } else { "local" };
    crate::manifests::cmd_apply(env_str, &env.domain, "", namespace).await?;

    // Pull fresh images into containerd on every node before rollout
    if let Some(imgs) = images {
        ctr_pull_on_nodes(env, imgs).await?;
    }

    for dep in deployments {
        ok(&format!("Rolling {dep}..."));
        crate::kube::kube_rollout_restart(namespace, dep).await?;
    }

    // Wait for rollout completion
    for dep in deployments {
        wait_deployment_ready(namespace, dep, timeout_secs).await?;
    }

    ok("Redeployed.");
    Ok(())
}

/// Wait for a deployment to become ready.
async fn wait_deployment_ready(ns: &str, deployment: &str, timeout_secs: u64) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use std::time::{Duration, Instant};

    let client = crate::kube::get_client().await?;
    let api: kube::api::Api<Deployment> = kube::api::Api::namespaced(client.clone(), ns);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if Instant::now() > deadline {
            return Err(SunbeamError::build(format!("Timed out waiting for deployment {ns}/{deployment}")));
        }

        if let Some(dep) = api.get_opt(deployment).await? {
            if let Some(status) = &dep.status {
                if let Some(conditions) = &status.conditions {
                    let available = conditions
                        .iter()
                        .any(|c| c.type_ == "Available" && c.status == "True");
                    if available {
                        return Ok(());
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// Mirroring
// ---------------------------------------------------------------------------

/// Docker Hub auth token response.
#[derive(serde::Deserialize)]
struct DockerAuthToken {
    token: String,
}

/// Fetch a Docker Hub auth token for the given repository.
async fn docker_hub_token(repo: &str) -> Result<String> {
    let url = format!(
        "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{repo}:pull"
    );
    let resp: DockerAuthToken = reqwest::get(&url)
        .await
        .ctx("Failed to fetch Docker Hub token")?
        .json()
        .await
        .ctx("Failed to parse Docker Hub token response")?;
    Ok(resp.token)
}

/// Fetch an OCI/Docker manifest index from Docker Hub.
async fn fetch_manifest_index(
    repo: &str,
    tag: &str,
) -> Result<serde_json::Value> {
    let token = docker_hub_token(repo).await?;

    let client = reqwest::Client::new();
    let url = format!("https://registry-1.docker.io/v2/{repo}/manifests/{tag}");
    let accept = "application/vnd.oci.image.index.v1+json,\
                   application/vnd.docker.distribution.manifest.list.v2+json";

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", accept)
        .send()
        .await
        .ctx("Failed to fetch manifest from Docker Hub")?;

    if !resp.status().is_success() {
        return Err(SunbeamError::build(format!(
            "Docker Hub returned {} for {repo}:{tag}",
            resp.status()
        )));
    }

    resp.json()
        .await
        .ctx("Failed to parse manifest index JSON")
}

/// Build an OCI tar archive containing a patched index that maps both
/// amd64 and arm64 to the same amd64 manifest.
fn make_oci_tar(
    ref_name: &str,
    new_index_bytes: &[u8],
    amd64_manifest_bytes: &[u8],
) -> Result<Vec<u8>> {
    use std::io::Write;

    let ix_hex = {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(new_index_bytes);
        hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };

    let new_index: serde_json::Value = serde_json::from_slice(new_index_bytes)?;
    let amd64_hex = new_index["manifests"][0]["digest"]
        .as_str()
        .unwrap_or("")
        .replace("sha256:", "");

    let layout = serde_json::json!({"imageLayoutVersion": "1.0.0"});
    let layout_bytes = serde_json::to_vec(&layout)?;

    let top = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "digest": format!("sha256:{ix_hex}"),
            "size": new_index_bytes.len(),
            "annotations": {
                "org.opencontainers.image.ref.name": ref_name,
            },
        }],
    });
    let top_bytes = serde_json::to_vec(&top)?;

    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);

        let mut add_entry = |name: &str, data: &[u8]| -> Result<()> {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, data)?;
            Ok(())
        };

        add_entry("oci-layout", &layout_bytes)?;
        add_entry("index.json", &top_bytes)?;
        add_entry(&format!("blobs/sha256/{ix_hex}"), new_index_bytes)?;
        add_entry(
            &format!("blobs/sha256/{amd64_hex}"),
            amd64_manifest_bytes,
        )?;

        builder.finish()?;
    }

    // Flush
    buf.flush().ok();
    Ok(buf)
}

/// Mirror amd64-only La Suite images to the Gitea registry.
///
/// The Python version ran a script inside the Lima VM via `limactl shell`.
/// Without Lima, we use reqwest for Docker registry token/manifest fetching
/// and construct OCI tars natively. The containerd import + push operations
/// require SSH to nodes and are implemented via subprocess.
pub async fn cmd_mirror() -> Result<()> {
    step("Mirroring amd64-only images to Gitea registry...");

    let domain = crate::kube::get_domain().await?;
    let admin_pass = crate::kube::kube_get_secret_field(
        "devtools",
        "gitea-admin-credentials",
        "password",
    )
    .await
    .unwrap_or_default();

    if admin_pass.is_empty() {
        warn("Could not get gitea admin password; skipping mirror.");
        return Ok(());
    }

    let registry = format!("src.{domain}");

    let nodes = get_node_addresses().await.unwrap_or_default();
    if nodes.is_empty() {
        warn("No node addresses found; cannot mirror images (need SSH to containerd).");
        return Ok(());
    }

    // Determine SSH user
    let ssh_host_val = crate::kube::ssh_host();
    let ssh_user = if ssh_host_val.contains('@') {
        ssh_host_val.split('@').next().unwrap_or("root")
    } else {
        "root"
    };

    for (src, org, repo, tag) in AMD64_ONLY_IMAGES {
        let tgt = format!("{registry}/{org}/{repo}:{tag}");
        ok(&format!("Processing {src} -> {tgt}"));

        // Fetch manifest index from Docker Hub
        let no_prefix = src.replace("docker.io/", "");
        let parts: Vec<&str> = no_prefix.splitn(2, ':').collect();
        let (docker_repo, docker_tag) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            (parts[0], "latest")
        };

        let index = match fetch_manifest_index(docker_repo, docker_tag).await {
            Ok(idx) => idx,
            Err(e) => {
                warn(&format!("Failed to fetch index for {src}: {e}"));
                continue;
            }
        };

        // Find amd64 manifest
        let manifests = index["manifests"].as_array();
        let amd64 = manifests.and_then(|ms| {
            ms.iter().find(|m| {
                m["platform"]["architecture"].as_str() == Some("amd64")
                    && m["platform"]["os"].as_str() == Some("linux")
            })
        });

        let amd64 = match amd64 {
            Some(m) => m.clone(),
            None => {
                warn(&format!("No linux/amd64 entry in index for {src}; skipping"));
                continue;
            }
        };

        let amd64_digest = amd64["digest"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // Fetch the actual amd64 manifest blob from registry
        let token = docker_hub_token(docker_repo).await?;
        let manifest_url = format!(
            "https://registry-1.docker.io/v2/{docker_repo}/manifests/{amd64_digest}"
        );
        let client = reqwest::Client::new();
        let amd64_manifest_bytes = client
            .get(&manifest_url)
            .header("Authorization", format!("Bearer {token}"))
            .header(
                "Accept",
                "application/vnd.oci.image.manifest.v1+json,\
                 application/vnd.docker.distribution.manifest.v2+json",
            )
            .send()
            .await?
            .bytes()
            .await?;

        // Build patched index: amd64 + arm64 alias pointing to same manifest
        let arm64_entry = serde_json::json!({
            "mediaType": amd64["mediaType"],
            "digest": amd64["digest"],
            "size": amd64["size"],
            "platform": {"architecture": "arm64", "os": "linux"},
        });

        let new_index = serde_json::json!({
            "schemaVersion": index["schemaVersion"],
            "mediaType": index.get("mediaType").unwrap_or(&serde_json::json!("application/vnd.oci.image.index.v1+json")),
            "manifests": [amd64, arm64_entry],
        });
        let new_index_bytes = serde_json::to_vec(&new_index)?;

        // Build OCI tar
        let oci_tar = match make_oci_tar(&tgt, &new_index_bytes, &amd64_manifest_bytes) {
            Ok(tar) => tar,
            Err(e) => {
                warn(&format!("Failed to build OCI tar for {tgt}: {e}"));
                continue;
            }
        };

        // Import + push via SSH to each node (containerd operations)
        for node_ip in &nodes {
            ok(&format!("Importing {tgt} on {node_ip}..."));

            // Remove existing, import, label
            let ssh_target = format!("{ssh_user}@{node_ip}");

            // Import via stdin
            let mut import_cmd = tokio::process::Command::new("ssh")
                .args([
                    "-p",
                    "2222",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &ssh_target,
                    "sudo ctr -n k8s.io images import --all-platforms -",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .ctx("Failed to spawn ssh for ctr import")?;

            if let Some(mut stdin) = import_cmd.stdin.take() {
                use tokio::io::AsyncWriteExt;
                stdin.write_all(&oci_tar).await?;
                drop(stdin);
            }
            let import_status = import_cmd.wait().await?;
            if !import_status.success() {
                warn(&format!("ctr import failed on {node_ip} for {tgt}"));
                continue;
            }

            // Label for CRI
            let _ = tokio::process::Command::new("ssh")
                .args([
                    "-p",
                    "2222",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &ssh_target,
                    &format!(
                        "sudo ctr -n k8s.io images label {tgt} io.cri-containerd.image=managed"
                    ),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;

            // Push to Gitea registry
            ok(&format!("Pushing {tgt} from {node_ip}..."));
            let push_status = tokio::process::Command::new("ssh")
                .args([
                    "-p",
                    "2222",
                    "-o",
                    "StrictHostKeyChecking=no",
                    &ssh_target,
                    &format!(
                        "sudo ctr -n k8s.io images push --user {GITEA_ADMIN_USER}:{admin_pass} {tgt}"
                    ),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .status()
                .await;

            match push_status {
                Ok(s) if s.success() => ok(&format!("Pushed {tgt}")),
                _ => warn(&format!("Push failed for {tgt} on {node_ip}")),
            }

            // Only need to push from one node
            break;
        }
    }

    // Delete pods stuck in image-pull error states
    ok("Clearing image-pull-error pods...");
    clear_image_pull_error_pods().await?;

    ok("Done.");
    Ok(())
}

/// Delete pods in image-pull error states across managed namespaces.
async fn clear_image_pull_error_pods() -> Result<()> {
    use k8s_openapi::api::core::v1::Pod;

    let error_reasons = ["ImagePullBackOff", "ErrImagePull", "ErrImageNeverPull"];

    let client = crate::kube::get_client().await?;

    for ns in MANAGED_NS {
        let api: kube::api::Api<Pod> = kube::api::Api::namespaced(client.clone(), ns);
        let pods = api
            .list(&kube::api::ListParams::default())
            .await;

        let pods = match pods {
            Ok(p) => p,
            Err(_) => continue,
        };

        for pod in &pods.items {
            let pod_name = pod.metadata.name.as_deref().unwrap_or("");
            if pod_name.is_empty() {
                continue;
            }

            let has_error = pod
                .status
                .as_ref()
                .and_then(|s| s.container_statuses.as_ref())
                .map(|statuses| {
                    statuses.iter().any(|cs| {
                        cs.state
                            .as_ref()
                            .and_then(|s| s.waiting.as_ref())
                            .and_then(|w| w.reason.as_deref())
                            .is_some_and(|r| error_reasons.contains(&r))
                    })
                })
                .unwrap_or(false);

            if has_error {
                let _ = api
                    .delete(pod_name, &kube::api::DeleteParams::default())
                    .await;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Build dispatch
// ---------------------------------------------------------------------------

/// Build an image. Pass push=true to push, deploy=true to also apply + rollout.
pub async fn cmd_build(what: &BuildTarget, push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    match what {
        BuildTarget::Proxy => builders::build_proxy(push, deploy, no_cache).await,
        BuildTarget::KratosAdmin => builders::build_kratos_admin(push, deploy, no_cache).await,
        BuildTarget::Tuwunel => builders::build_tuwunel(push, deploy, no_cache).await,
        BuildTarget::Sol => builders::build_sol(push, deploy, no_cache).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_ns_is_sorted() {
        let mut sorted = MANAGED_NS.to_vec();
        sorted.sort();
        assert_eq!(
            MANAGED_NS, &sorted[..],
            "MANAGED_NS should be in alphabetical order"
        );
    }

    #[test]
    fn managed_ns_contains_expected_namespaces() {
        assert!(MANAGED_NS.contains(&"data"));
        assert!(MANAGED_NS.contains(&"devtools"));
        assert!(MANAGED_NS.contains(&"ingress"));
        assert!(MANAGED_NS.contains(&"ory"));
        assert!(MANAGED_NS.contains(&"matrix"));
    }

    #[test]
    fn amd64_only_images_empty_after_lasuite_removal() {
        assert!(AMD64_ONLY_IMAGES.is_empty());
    }

    #[test]
    fn build_target_display_proxy() {
        assert_eq!(BuildTarget::Proxy.to_string(), "proxy");
    }

    #[test]
    fn build_target_display_kratos_admin() {
        assert_eq!(BuildTarget::KratosAdmin.to_string(), "kratos-admin");
    }

    #[test]
    fn build_target_display_all_lowercase_or_hyphenated() {
        let targets = [
            BuildTarget::Proxy,
            BuildTarget::KratosAdmin,
            BuildTarget::Tuwunel,
            BuildTarget::Sol,
        ];
        for t in &targets {
            let s = t.to_string();
            assert!(
                s.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "BuildTarget display '{s}' has unexpected characters"
            );
        }
    }

    #[test]
    fn gitea_admin_user_constant() {
        assert_eq!(GITEA_ADMIN_USER, "gitea_admin");
    }

}
