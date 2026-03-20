//! Image building, mirroring, and pushing to Gitea registry.

use crate::error::{Result, ResultExt, SunbeamError};
use base64::Engine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::cli::BuildTarget;
use crate::output::{ok, step, warn};

const GITEA_ADMIN_USER: &str = "gitea_admin";

const MANAGED_NS: &[&str] = &[
    "data",
    "devtools",
    "ingress",
    "lasuite",
    "matrix",
    "media",
    "ory",
    "storage",
    "vault-secrets-operator",
];

/// amd64-only images that need mirroring: (source, org, repo, tag).
const AMD64_ONLY_IMAGES: &[(&str, &str, &str, &str)] = &[
    (
        "docker.io/lasuite/people-backend:latest",
        "studio",
        "people-backend",
        "latest",
    ),
    (
        "docker.io/lasuite/people-frontend:latest",
        "studio",
        "people-frontend",
        "latest",
    ),
    (
        "docker.io/lasuite/impress-backend:latest",
        "studio",
        "impress-backend",
        "latest",
    ),
    (
        "docker.io/lasuite/impress-frontend:latest",
        "studio",
        "impress-frontend",
        "latest",
    ),
    (
        "docker.io/lasuite/impress-y-provider:latest",
        "studio",
        "impress-y-provider",
        "latest",
    ),
];

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
async fn get_build_env() -> Result<BuildEnv> {
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
async fn buildctl_build_and_push(
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
async fn build_image(
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
async fn ctr_pull_on_nodes(env: &BuildEnv, images: &[String]) -> Result<()> {
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
async fn deploy_rollout(
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
// Per-service build functions
// ---------------------------------------------------------------------------

async fn build_proxy(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let proxy_dir = crate::config::get_repo_root().join("proxy");
    if !proxy_dir.is_dir() {
        return Err(SunbeamError::build(format!("Proxy source not found at {}", proxy_dir.display())));
    }

    let image = format!("{}/studio/proxy:latest", env.registry);
    step(&format!("Building sunbeam-proxy -> {image} ..."));

    build_image(
        &env,
        &image,
        &proxy_dir.join("Dockerfile"),
        &proxy_dir,
        None,
        None,
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["pingora"], "ingress", 120, Some(&[image])).await?;
    }
    Ok(())
}

async fn build_tuwunel(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let tuwunel_dir = crate::config::get_repo_root().join("tuwunel");
    if !tuwunel_dir.is_dir() {
        return Err(SunbeamError::build(format!("Tuwunel source not found at {}", tuwunel_dir.display())));
    }

    let image = format!("{}/studio/tuwunel:latest", env.registry);
    step(&format!("Building tuwunel -> {image} ..."));

    build_image(
        &env,
        &image,
        &tuwunel_dir.join("Dockerfile"),
        &tuwunel_dir,
        None,
        None,
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["tuwunel"], "matrix", 180, Some(&[image])).await?;
    }
    Ok(())
}

async fn build_integration(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let sunbeam_dir = crate::config::get_repo_root();
    let integration_service_dir = sunbeam_dir.join("integration-service");
    let dockerfile = integration_service_dir.join("Dockerfile");
    let dockerignore = integration_service_dir.join(".dockerignore");

    if !dockerfile.exists() {
        return Err(SunbeamError::build(format!(
            "integration-service Dockerfile not found at {}",
            dockerfile.display()
        )));
    }
    if !sunbeam_dir
        .join("integration")
        .join("packages")
        .join("widgets")
        .is_dir()
    {
        return Err(SunbeamError::build(format!(
            "integration repo not found at {} -- \
             run: cd sunbeam && git clone https://github.com/suitenumerique/integration.git",
            sunbeam_dir.join("integration").display()
        )));
    }

    let image = format!("{}/studio/integration:latest", env.registry);
    step(&format!("Building integration -> {image} ..."));

    // .dockerignore needs to be at context root
    let root_ignore = sunbeam_dir.join(".dockerignore");
    let mut copied_ignore = false;
    if !root_ignore.exists() && dockerignore.exists() {
        std::fs::copy(&dockerignore, &root_ignore).ok();
        copied_ignore = true;
    }

    let result = build_image(
        &env,
        &image,
        &dockerfile,
        &sunbeam_dir,
        None,
        None,
        push,
        false,
        &[],
    )
    .await;

    if copied_ignore && root_ignore.exists() {
        let _ = std::fs::remove_file(&root_ignore);
    }

    result?;

    if deploy {
        deploy_rollout(&env, &["integration"], "lasuite", 120, None).await?;
    }
    Ok(())
}

async fn build_kratos_admin(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let kratos_admin_dir = crate::config::get_repo_root().join("kratos-admin");
    if !kratos_admin_dir.is_dir() {
        return Err(SunbeamError::build(format!(
            "kratos-admin source not found at {}",
            kratos_admin_dir.display()
        )));
    }

    let image = format!("{}/studio/kratos-admin-ui:latest", env.registry);
    step(&format!("Building kratos-admin-ui -> {image} ..."));

    build_image(
        &env,
        &image,
        &kratos_admin_dir.join("Dockerfile"),
        &kratos_admin_dir,
        None,
        None,
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["kratos-admin-ui"], "ory", 120, None).await?;
    }
    Ok(())
}

async fn build_meet(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let meet_dir = crate::config::get_repo_root().join("meet");
    if !meet_dir.is_dir() {
        return Err(SunbeamError::build(format!("meet source not found at {}", meet_dir.display())));
    }

    let backend_image = format!("{}/studio/meet-backend:latest", env.registry);
    let frontend_image = format!("{}/studio/meet-frontend:latest", env.registry);

    // Backend
    step(&format!("Building meet-backend -> {backend_image} ..."));
    build_image(
        &env,
        &backend_image,
        &meet_dir.join("Dockerfile"),
        &meet_dir,
        Some("backend-production"),
        None,
        push,
        false,
        &[],
    )
    .await?;

    // Frontend
    step(&format!("Building meet-frontend -> {frontend_image} ..."));
    let frontend_dockerfile = meet_dir.join("src").join("frontend").join("Dockerfile");
    if !frontend_dockerfile.exists() {
        return Err(SunbeamError::build(format!(
            "meet frontend Dockerfile not found at {}",
            frontend_dockerfile.display()
        )));
    }

    let mut build_args = HashMap::new();
    build_args.insert("VITE_API_BASE_URL".to_string(), String::new());

    build_image(
        &env,
        &frontend_image,
        &frontend_dockerfile,
        &meet_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(
            &env,
            &["meet-backend", "meet-celery-worker", "meet-frontend"],
            "lasuite",
            180,
            None,
        )
        .await?;
    }
    Ok(())
}

async fn build_people(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let people_dir = crate::config::get_repo_root().join("people");
    if !people_dir.is_dir() {
        return Err(SunbeamError::build(format!("people source not found at {}", people_dir.display())));
    }

    let workspace_dir = people_dir.join("src").join("frontend");
    let app_dir = workspace_dir.join("apps").join("desk");
    let dockerfile = workspace_dir.join("Dockerfile");
    if !dockerfile.exists() {
        return Err(SunbeamError::build(format!("Dockerfile not found at {}", dockerfile.display())));
    }

    let image = format!("{}/studio/people-frontend:latest", env.registry);
    step(&format!("Building people-frontend -> {image} ..."));

    // yarn install
    ok("Updating yarn.lock (yarn install in workspace)...");
    let yarn_status = tokio::process::Command::new("yarn")
        .args(["install", "--ignore-engines"])
        .current_dir(&workspace_dir)
        .status()
        .await
        .ctx("Failed to run yarn install")?;
    if !yarn_status.success() {
        return Err(SunbeamError::tool("yarn", "install failed"));
    }

    // cunningham design tokens
    ok("Regenerating cunningham design tokens...");
    let cunningham_bin = workspace_dir
        .join("node_modules")
        .join(".bin")
        .join("cunningham");
    let cunningham_status = tokio::process::Command::new(&cunningham_bin)
        .args(["-g", "css,ts", "-o", "src/cunningham", "--utility-classes"])
        .current_dir(&app_dir)
        .status()
        .await
        .ctx("Failed to run cunningham")?;
    if !cunningham_status.success() {
        return Err(SunbeamError::tool("cunningham", "design token generation failed"));
    }

    let mut build_args = HashMap::new();
    build_args.insert("DOCKER_USER".to_string(), "101".to_string());

    build_image(
        &env,
        &image,
        &dockerfile,
        &people_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["people-frontend"], "lasuite", 180, None).await?;
    }
    Ok(())
}

/// Message component definition: (cli_name, image_name, dockerfile_rel, target).
const MESSAGES_COMPONENTS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "messages-backend",
        "messages-backend",
        "src/backend/Dockerfile",
        Some("runtime-distroless-prod"),
    ),
    (
        "messages-frontend",
        "messages-frontend",
        "src/frontend/Dockerfile",
        Some("runtime-prod"),
    ),
    (
        "messages-mta-in",
        "messages-mta-in",
        "src/mta-in/Dockerfile",
        None,
    ),
    (
        "messages-mta-out",
        "messages-mta-out",
        "src/mta-out/Dockerfile",
        None,
    ),
    (
        "messages-mpa",
        "messages-mpa",
        "src/mpa/rspamd/Dockerfile",
        None,
    ),
    (
        "messages-socks-proxy",
        "messages-socks-proxy",
        "src/socks-proxy/Dockerfile",
        None,
    ),
];

async fn build_messages(what: &str, push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let messages_dir = crate::config::get_repo_root().join("messages");
    if !messages_dir.is_dir() {
        return Err(SunbeamError::build(format!("messages source not found at {}", messages_dir.display())));
    }

    let components: Vec<_> = if what == "messages" {
        MESSAGES_COMPONENTS.to_vec()
    } else {
        MESSAGES_COMPONENTS
            .iter()
            .filter(|(name, _, _, _)| *name == what)
            .copied()
            .collect()
    };

    let mut built_images = Vec::new();

    for (component, image_name, dockerfile_rel, target) in &components {
        let dockerfile = messages_dir.join(dockerfile_rel);
        if !dockerfile.exists() {
            warn(&format!(
                "Dockerfile not found at {} -- skipping {component}",
                dockerfile.display()
            ));
            continue;
        }

        let image = format!("{}/studio/{image_name}:latest", env.registry);
        let context_dir = dockerfile.parent().unwrap_or(&messages_dir);
        step(&format!("Building {component} -> {image} ..."));

        // Patch ghcr.io/astral-sh/uv COPY for messages-backend on local builds
        let mut cleanup_paths = Vec::new();
        let actual_dockerfile;

        if !env.is_prod && *image_name == "messages-backend" {
            let (patched, cleanup) =
                patch_dockerfile_uv(&dockerfile, context_dir, &env.platform).await?;
            actual_dockerfile = patched;
            cleanup_paths = cleanup;
        } else {
            actual_dockerfile = dockerfile.clone();
        }

        build_image(
            &env,
            &image,
            &actual_dockerfile,
            context_dir,
            *target,
            None,
            push,
            false,
            &cleanup_paths,
        )
        .await?;

        built_images.push(image);
    }

    if deploy && !built_images.is_empty() {
        deploy_rollout(
            &env,
            &[
                "messages-backend",
                "messages-worker",
                "messages-frontend",
                "messages-mta-in",
                "messages-mta-out",
                "messages-mpa",
                "messages-socks-proxy",
            ],
            "lasuite",
            180,
            None,
        )
        .await?;
    }

    Ok(())
}

/// Build a La Suite frontend image from source and push to the Gitea registry.
#[allow(clippy::too_many_arguments)]
async fn build_la_suite_frontend(
    app: &str,
    repo_dir: &Path,
    workspace_rel: &str,
    app_rel: &str,
    dockerfile_rel: &str,
    image_name: &str,
    deployment: &str,
    namespace: &str,
    push: bool,
    deploy: bool,
) -> Result<()> {
    let env = get_build_env().await?;

    let workspace_dir = repo_dir.join(workspace_rel);
    let app_dir = repo_dir.join(app_rel);
    let dockerfile = repo_dir.join(dockerfile_rel);

    if !repo_dir.is_dir() {
        return Err(SunbeamError::build(format!("{app} source not found at {}", repo_dir.display())));
    }
    if !dockerfile.exists() {
        return Err(SunbeamError::build(format!("Dockerfile not found at {}", dockerfile.display())));
    }

    let image = format!("{}/studio/{image_name}:latest", env.registry);
    step(&format!("Building {app} -> {image} ..."));

    ok("Updating yarn.lock (yarn install in workspace)...");
    let yarn_status = tokio::process::Command::new("yarn")
        .args(["install", "--ignore-engines"])
        .current_dir(&workspace_dir)
        .status()
        .await
        .ctx("Failed to run yarn install")?;
    if !yarn_status.success() {
        return Err(SunbeamError::tool("yarn", "install failed"));
    }

    ok("Regenerating cunningham design tokens (yarn build-theme)...");
    let theme_status = tokio::process::Command::new("yarn")
        .args(["build-theme"])
        .current_dir(&app_dir)
        .status()
        .await
        .ctx("Failed to run yarn build-theme")?;
    if !theme_status.success() {
        return Err(SunbeamError::tool("yarn", "build-theme failed"));
    }

    let mut build_args = HashMap::new();
    build_args.insert("DOCKER_USER".to_string(), "101".to_string());

    build_image(
        &env,
        &image,
        &dockerfile,
        repo_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &[deployment], namespace, 180, None).await?;
    }
    Ok(())
}

/// Download uv from GitHub releases and return a patched Dockerfile path.
async fn patch_dockerfile_uv(
    dockerfile_path: &Path,
    context_dir: &Path,
    platform: &str,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let content = std::fs::read_to_string(dockerfile_path)
        .ctx("Failed to read Dockerfile for uv patching")?;

    // Match COPY --from=ghcr.io/astral-sh/uv@sha256:... /uv /uvx /bin/
    let original_copy = content
        .lines()
        .find(|line| {
            line.contains("COPY")
                && line.contains("--from=ghcr.io/astral-sh/uv@sha256:")
                && line.contains("/uv")
                && line.contains("/bin/")
        })
        .map(|line| line.trim().to_string());

    let original_copy = match original_copy {
        Some(c) => c,
        None => return Ok((dockerfile_path.to_path_buf(), vec![])),
    };

    // Find uv version from comment like: oci://ghcr.io/astral-sh/uv:0.x.y
    let version = content
        .lines()
        .find_map(|line| {
            let marker = "oci://ghcr.io/astral-sh/uv:";
            if let Some(idx) = line.find(marker) {
                let rest = &line[idx + marker.len()..];
                let ver = rest.split_whitespace().next().unwrap_or("");
                if !ver.is_empty() {
                    Some(ver.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        });

    let version = match version {
        Some(v) => v,
        None => {
            warn("Could not find uv version comment in Dockerfile; ghcr.io pull may fail.");
            return Ok((dockerfile_path.to_path_buf(), vec![]));
        }
    };

    let arch = if platform.contains("amd64") {
        "x86_64"
    } else {
        "aarch64"
    };

    let url = format!(
        "https://github.com/astral-sh/uv/releases/download/{version}/uv-{arch}-unknown-linux-gnu.tar.gz"
    );

    let stage_dir = context_dir.join("_sunbeam_uv_stage");
    let patched_df = dockerfile_path
        .parent()
        .unwrap_or(dockerfile_path)
        .join("Dockerfile._sunbeam_patched");
    let cleanup = vec![stage_dir.clone(), patched_df.clone()];

    ok(&format!(
        "Downloading uv {version} ({arch}) from GitHub releases to bypass ghcr.io..."
    ));

    std::fs::create_dir_all(&stage_dir)?;

    // Download tarball
    let response = reqwest::get(&url)
        .await
        .ctx("Failed to download uv release")?;
    let tarball_bytes = response.bytes().await?;

    // Extract uv and uvx from tarball
    let decoder = flate2::read::GzDecoder::new(&tarball_bytes[..]);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if (file_name == "uv" || file_name == "uvx") && entry.header().entry_type().is_file() {
            let dest = stage_dir.join(&file_name);
            let mut outfile = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut outfile)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }

    if !stage_dir.join("uv").exists() {
        warn("uv binary not found in release tarball; build may fail.");
        return Ok((dockerfile_path.to_path_buf(), cleanup));
    }

    let patched = content.replace(
        &original_copy,
        "COPY _sunbeam_uv_stage/uv _sunbeam_uv_stage/uvx /bin/",
    );
    std::fs::write(&patched_df, patched)?;
    ok(&format!("  uv {version} staged; using patched Dockerfile."));

    Ok((patched_df, cleanup))
}

async fn build_projects(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let projects_dir = crate::config::get_repo_root().join("projects");
    if !projects_dir.is_dir() {
        return Err(SunbeamError::build(format!("projects source not found at {}", projects_dir.display())));
    }

    let image = format!("{}/studio/projects:latest", env.registry);
    step(&format!("Building projects -> {image} ..."));

    build_image(
        &env,
        &image,
        &projects_dir.join("Dockerfile"),
        &projects_dir,
        None,
        None,
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["projects"], "lasuite", 180, Some(&[image])).await?;
    }
    Ok(())
}

async fn build_calendars(push: bool, deploy: bool) -> Result<()> {
    let env = get_build_env().await?;
    let cal_dir = crate::config::get_repo_root().join("calendars");
    if !cal_dir.is_dir() {
        return Err(SunbeamError::build(format!("calendars source not found at {}", cal_dir.display())));
    }

    let backend_dir = cal_dir.join("src").join("backend");
    let backend_image = format!("{}/studio/calendars-backend:latest", env.registry);
    step(&format!("Building calendars-backend -> {backend_image} ..."));

    // Stage translations.json into the build context
    let translations_src = cal_dir
        .join("src")
        .join("frontend")
        .join("apps")
        .join("calendars")
        .join("src")
        .join("features")
        .join("i18n")
        .join("translations.json");

    let translations_dst = backend_dir.join("_translations.json");
    let mut cleanup: Vec<PathBuf> = Vec::new();
    let mut dockerfile = backend_dir.join("Dockerfile");

    if translations_src.exists() {
        std::fs::copy(&translations_src, &translations_dst)?;
        cleanup.push(translations_dst);

        // Patch Dockerfile to COPY translations into production image
        let mut content = std::fs::read_to_string(&dockerfile)?;
        content.push_str(
            "\n# Sunbeam: bake translations.json for default calendar names\n\
             COPY _translations.json /data/translations.json\n",
        );
        let patched_df = backend_dir.join("Dockerfile._sunbeam_patched");
        std::fs::write(&patched_df, content)?;
        cleanup.push(patched_df.clone());
        dockerfile = patched_df;
    }

    build_image(
        &env,
        &backend_image,
        &dockerfile,
        &backend_dir,
        Some("backend-production"),
        None,
        push,
        false,
        &cleanup,
    )
    .await?;

    // caldav
    let caldav_image = format!("{}/studio/calendars-caldav:latest", env.registry);
    step(&format!("Building calendars-caldav -> {caldav_image} ..."));
    let caldav_dir = cal_dir.join("src").join("caldav");
    build_image(
        &env,
        &caldav_image,
        &caldav_dir.join("Dockerfile"),
        &caldav_dir,
        None,
        None,
        push,
        false,
        &[],
    )
    .await?;

    // frontend
    let frontend_image = format!("{}/studio/calendars-frontend:latest", env.registry);
    step(&format!(
        "Building calendars-frontend -> {frontend_image} ..."
    ));
    let integration_base = format!("https://integration.{}", env.domain);
    let mut build_args = HashMap::new();
    build_args.insert(
        "VISIO_BASE_URL".to_string(),
        format!("https://meet.{}", env.domain),
    );
    build_args.insert(
        "GAUFRE_WIDGET_PATH".to_string(),
        format!("{integration_base}/api/v2/lagaufre.js"),
    );
    build_args.insert(
        "GAUFRE_API_URL".to_string(),
        format!("{integration_base}/api/v2/services.json"),
    );
    build_args.insert(
        "THEME_CSS_URL".to_string(),
        format!("{integration_base}/api/v2/theme.css"),
    );

    let frontend_dir = cal_dir.join("src").join("frontend");
    build_image(
        &env,
        &frontend_image,
        &frontend_dir.join("Dockerfile"),
        &frontend_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        false,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(
            &env,
            &[
                "calendars-backend",
                "calendars-worker",
                "calendars-caldav",
                "calendars-frontend",
            ],
            "lasuite",
            180,
            Some(&[backend_image, caldav_image, frontend_image]),
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Build dispatch
// ---------------------------------------------------------------------------

/// Build an image. Pass push=true to push, deploy=true to also apply + rollout.
pub async fn cmd_build(what: &BuildTarget, push: bool, deploy: bool) -> Result<()> {
    match what {
        BuildTarget::Proxy => build_proxy(push, deploy).await,
        BuildTarget::Integration => build_integration(push, deploy).await,
        BuildTarget::KratosAdmin => build_kratos_admin(push, deploy).await,
        BuildTarget::Meet => build_meet(push, deploy).await,
        BuildTarget::DocsFrontend => {
            let repo_dir = crate::config::get_repo_root().join("docs");
            build_la_suite_frontend(
                "docs-frontend",
                &repo_dir,
                "src/frontend",
                "src/frontend/apps/impress",
                "src/frontend/Dockerfile",
                "impress-frontend",
                "docs-frontend",
                "lasuite",
                push,
                deploy,
            )
            .await
        }
        BuildTarget::PeopleFrontend | BuildTarget::People => build_people(push, deploy).await,
        BuildTarget::Messages => build_messages("messages", push, deploy).await,
        BuildTarget::MessagesBackend => build_messages("messages-backend", push, deploy).await,
        BuildTarget::MessagesFrontend => build_messages("messages-frontend", push, deploy).await,
        BuildTarget::MessagesMtaIn => build_messages("messages-mta-in", push, deploy).await,
        BuildTarget::MessagesMtaOut => build_messages("messages-mta-out", push, deploy).await,
        BuildTarget::MessagesMpa => build_messages("messages-mpa", push, deploy).await,
        BuildTarget::MessagesSocksProxy => {
            build_messages("messages-socks-proxy", push, deploy).await
        }
        BuildTarget::Tuwunel => build_tuwunel(push, deploy).await,
        BuildTarget::Calendars => build_calendars(push, deploy).await,
        BuildTarget::Projects => build_projects(push, deploy).await,
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
    fn amd64_only_images_all_from_docker_hub() {
        for (src, _org, _repo, _tag) in AMD64_ONLY_IMAGES {
            assert!(
                src.starts_with("docker.io/"),
                "Expected docker.io prefix, got: {src}"
            );
        }
    }

    #[test]
    fn amd64_only_images_all_have_latest_tag() {
        for (src, _org, _repo, tag) in AMD64_ONLY_IMAGES {
            assert_eq!(
                *tag, "latest",
                "Expected 'latest' tag for {src}, got: {tag}"
            );
        }
    }

    #[test]
    fn amd64_only_images_non_empty() {
        assert!(
            !AMD64_ONLY_IMAGES.is_empty(),
            "AMD64_ONLY_IMAGES should not be empty"
        );
    }

    #[test]
    fn amd64_only_images_org_is_studio() {
        for (src, org, _repo, _tag) in AMD64_ONLY_IMAGES {
            assert_eq!(
                *org, "studio",
                "Expected org 'studio' for {src}, got: {org}"
            );
        }
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
            BuildTarget::Integration,
            BuildTarget::KratosAdmin,
            BuildTarget::Meet,
            BuildTarget::DocsFrontend,
            BuildTarget::PeopleFrontend,
            BuildTarget::People,
            BuildTarget::Messages,
            BuildTarget::MessagesBackend,
            BuildTarget::MessagesFrontend,
            BuildTarget::MessagesMtaIn,
            BuildTarget::MessagesMtaOut,
            BuildTarget::MessagesMpa,
            BuildTarget::MessagesSocksProxy,
            BuildTarget::Tuwunel,
            BuildTarget::Calendars,
            BuildTarget::Projects,
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

    #[test]
    fn messages_components_non_empty() {
        assert!(!MESSAGES_COMPONENTS.is_empty());
    }

    #[test]
    fn messages_components_dockerfiles_are_relative() {
        for (_name, _image, dockerfile_rel, _target) in MESSAGES_COMPONENTS {
            assert!(
                dockerfile_rel.ends_with("Dockerfile"),
                "Expected Dockerfile suffix in: {dockerfile_rel}"
            );
            assert!(
                !dockerfile_rel.starts_with('/'),
                "Dockerfile path should be relative: {dockerfile_rel}"
            );
        }
    }

    #[test]
    fn messages_components_names_match_build_targets() {
        for (name, _image, _df, _target) in MESSAGES_COMPONENTS {
            assert!(
                name.starts_with("messages-"),
                "Component name should start with 'messages-': {name}"
            );
        }
    }
}
