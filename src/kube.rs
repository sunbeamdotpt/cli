use crate::error::{Result, SunbeamError, ResultExt};
use base64::Engine;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::api::{Api, ApiResource, DynamicObject, ListParams, Patch, PatchParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::{self, Scope};
use kube::{Client, Config};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tokio::sync::OnceCell;

static CONTEXT: OnceLock<String> = OnceLock::new();
static SSH_HOST: OnceLock<String> = OnceLock::new();
static KUBE_CLIENT: OnceCell<Client> = OnceCell::const_new();
static SSH_TUNNEL: Mutex<Option<tokio::process::Child>> = Mutex::new(None);
static API_DISCOVERY: OnceCell<kube::discovery::Discovery> = OnceCell::const_new();

/// Set the active kubectl context and optional SSH host for production tunnel.
pub fn set_context(ctx: &str, ssh_host: &str) {
    let _ = CONTEXT.set(ctx.to_string());
    let _ = SSH_HOST.set(ssh_host.to_string());
}

/// Get the active context.
pub fn context() -> &'static str {
    CONTEXT.get().map(|s| s.as_str()).unwrap_or("sunbeam")
}

/// Get the SSH host (empty for local).
pub fn ssh_host() -> &'static str {
    SSH_HOST.get().map(|s| s.as_str()).unwrap_or("")
}

// ---------------------------------------------------------------------------
// SSH tunnel management
// ---------------------------------------------------------------------------

/// Ensure SSH tunnel is open for production (forwards localhost:16443 -> remote:6443).
/// For local dev (empty ssh_host), this is a no-op.
#[allow(dead_code)]
pub async fn ensure_tunnel() -> Result<()> {
    let host = ssh_host();
    if host.is_empty() {
        return Ok(());
    }

    // Check if tunnel is already open
    if tokio::net::TcpStream::connect("127.0.0.1:16443")
        .await
        .is_ok()
    {
        return Ok(());
    }

    crate::output::ok(&format!("Opening SSH tunnel to {host}..."));

    let child = tokio::process::Command::new("ssh")
        .args([
            "-p",
            "2222",
            "-L",
            "16443:127.0.0.1:6443",
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "StrictHostKeyChecking=no",
            host,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ctx("Failed to spawn SSH tunnel")?;

    // Store child so it lives for the process lifetime (and can be killed on cleanup)
    if let Ok(mut guard) = SSH_TUNNEL.lock() {
        *guard = Some(child);
    }

    // Wait for tunnel to become available
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if tokio::net::TcpStream::connect("127.0.0.1:16443")
            .await
            .is_ok()
        {
            return Ok(());
        }
    }

    bail!("SSH tunnel to {host} did not open in time")
}

// ---------------------------------------------------------------------------
// Client initialization
// ---------------------------------------------------------------------------

/// Get or create a kube::Client configured for the active context.
/// Opens SSH tunnel first if needed for production.
pub async fn get_client() -> Result<&'static Client> {
    KUBE_CLIENT
        .get_or_try_init(|| async {
            ensure_tunnel().await?;

            let kubeconfig = Kubeconfig::read().map_err(|e| SunbeamError::kube(format!("Failed to read kubeconfig: {e}")))?;
            let options = KubeConfigOptions {
                context: Some(context().to_string()),
                ..Default::default()
            };
            let config = Config::from_custom_kubeconfig(kubeconfig, &options)
                .await
                .map_err(|e| SunbeamError::kube(format!("Failed to build kube config from kubeconfig: {e}")))?;
            Client::try_from(config).ctx("Failed to create kube client")
        })
        .await
}

// ---------------------------------------------------------------------------
// Core Kubernetes operations
// ---------------------------------------------------------------------------

/// Server-side apply a multi-document YAML manifest.
#[allow(dead_code)]
pub async fn kube_apply(manifest: &str) -> Result<()> {
    let client = get_client().await?;
    let ssapply = PatchParams::apply("sunbeam").force();

    for doc in manifest.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() || doc == "---" {
            continue;
        }

        // Parse the YAML to a DynamicObject so we can route it
        let obj: serde_yaml::Value =
            serde_yaml::from_str(doc).ctx("Failed to parse YAML document")?;

        let api_version = obj
            .get("apiVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let metadata = obj.get("metadata");
        let name = metadata
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let namespace = metadata
            .and_then(|m| m.get("namespace"))
            .and_then(|v| v.as_str());

        if name.is_empty() || kind.is_empty() {
            continue; // skip incomplete documents
        }

        // Use discovery to find the right API resource
        let (ar, scope) = resolve_api_resource(client, api_version, kind).await?;

        let api: Api<DynamicObject> = if let Some(ns) = namespace {
            Api::namespaced_with(client.clone(), ns, &ar)
        } else if scope == Scope::Namespaced {
            // Namespaced resource without a namespace specified; use default
            Api::default_namespaced_with(client.clone(), &ar)
        } else {
            Api::all_with(client.clone(), &ar)
        };

        let patch: serde_json::Value =
            serde_yaml::from_str(doc).ctx("Failed to parse YAML to JSON value")?;

        api.patch(name, &ssapply, &Patch::Apply(patch))
            .await
            .with_ctx(|| format!("Failed to apply {kind}/{name}"))?;
    }
    Ok(())
}

/// Resolve an API resource from apiVersion and kind using discovery.
async fn resolve_api_resource(
    client: &Client,
    api_version: &str,
    kind: &str,
) -> Result<(ApiResource, Scope)> {
    // Split apiVersion into group and version
    let (group, version) = if api_version.contains('/') {
        let parts: Vec<&str> = api_version.splitn(2, '/').collect();
        (parts[0], parts[1])
    } else {
        ("", api_version) // core API group
    };

    let disc = API_DISCOVERY
        .get_or_try_init(|| async {
            discovery::Discovery::new(client.clone())
                .run()
                .await
                .ctx("API discovery failed")
        })
        .await?;

    for api_group in disc.groups() {
        if api_group.name() == group {
            for (ar, caps) in api_group.resources_by_stability() {
                if ar.kind == kind && ar.version == version {
                    return Ok((ar, caps.scope));
                }
            }
        }
    }

    bail!("Could not discover API resource for {api_version}/{kind}")
}

/// Get a Kubernetes Secret object.
#[allow(dead_code)]
pub async fn kube_get_secret(ns: &str, name: &str) -> Result<Option<Secret>> {
    let client = get_client().await?;
    let api: Api<Secret> = Api::namespaced(client.clone(), ns);
    match api.get_opt(name).await {
        Ok(secret) => Ok(secret),
        Err(e) => Err(e).with_ctx(|| format!("Failed to get secret {ns}/{name}")),
    }
}

/// Get a specific base64-decoded field from a Kubernetes secret.
#[allow(dead_code)]
pub async fn kube_get_secret_field(ns: &str, name: &str, key: &str) -> Result<String> {
    let secret = kube_get_secret(ns, name)
        .await?
        .with_ctx(|| format!("Secret {ns}/{name} not found"))?;

    let data = secret.data.as_ref().ctx("Secret has no data")?;

    let bytes = data
        .get(key)
        .with_ctx(|| format!("Key {key:?} not found in secret {ns}/{name}"))?;

    String::from_utf8(bytes.0.clone())
        .with_ctx(|| format!("Key {key:?} in secret {ns}/{name} is not valid UTF-8"))
}

/// Check if a namespace exists.
#[allow(dead_code)]
pub async fn ns_exists(ns: &str) -> Result<bool> {
    let client = get_client().await?;
    let api: Api<Namespace> = Api::all(client.clone());
    match api.get_opt(ns).await {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(e) => Err(e).with_ctx(|| format!("Failed to check namespace {ns}")),
    }
}

/// Create namespace if it does not exist.
#[allow(dead_code)]
pub async fn ensure_ns(ns: &str) -> Result<()> {
    if ns_exists(ns).await? {
        return Ok(());
    }
    let client = get_client().await?;
    let api: Api<Namespace> = Api::all(client.clone());
    let ns_obj = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": { "name": ns }
    });
    let pp = PatchParams::apply("sunbeam").force();
    api.patch(ns, &pp, &Patch::Apply(ns_obj))
        .await
        .with_ctx(|| format!("Failed to create namespace {ns}"))?;
    Ok(())
}

/// Create or update a generic Kubernetes secret via server-side apply.
#[allow(dead_code)]
pub async fn create_secret(ns: &str, name: &str, data: HashMap<String, String>) -> Result<()> {
    let client = get_client().await?;
    let api: Api<Secret> = Api::namespaced(client.clone(), ns);

    // Encode values as base64
    let mut encoded: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for (k, v) in &data {
        let b64 = base64::engine::general_purpose::STANDARD.encode(v.as_bytes());
        encoded.insert(k.clone(), serde_json::Value::String(b64));
    }

    let secret_obj = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": name,
            "namespace": ns,
        },
        "type": "Opaque",
        "data": encoded,
    });

    let pp = PatchParams::apply("sunbeam").force();
    api.patch(name, &pp, &Patch::Apply(secret_obj))
        .await
        .with_ctx(|| format!("Failed to create/update secret {ns}/{name}"))?;
    Ok(())
}

/// Execute a command in a pod and return (exit_code, stdout).
#[allow(dead_code)]
pub async fn kube_exec(
    ns: &str,
    pod: &str,
    cmd: &[&str],
    container: Option<&str>,
) -> Result<(i32, String)> {
    let client = get_client().await?;
    let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client.clone(), ns);

    let mut ep = kube::api::AttachParams::default();
    ep.stdout = true;
    ep.stderr = true;
    ep.stdin = false;
    if let Some(c) = container {
        ep.container = Some(c.to_string());
    }

    let cmd_strings: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
    let mut attached = pods
        .exec(pod, cmd_strings, &ep)
        .await
        .with_ctx(|| format!("Failed to exec in pod {ns}/{pod}"))?;

    let stdout = {
        let mut stdout_reader = attached
            .stdout()
            .ctx("No stdout stream from exec")?;
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stdout_reader, &mut buf).await?;
        String::from_utf8_lossy(&buf).to_string()
    };

    let status = attached
        .take_status()
        .ctx("No status channel from exec")?;

    // Wait for the status
    let exit_code = if let Some(status) = status.await {
        status
            .status
            .map(|s| if s == "Success" { 0 } else { 1 })
            .unwrap_or(1)
    } else {
        1
    };

    Ok((exit_code, stdout.trim().to_string()))
}

/// Patch a deployment to trigger a rollout restart.
#[allow(dead_code)]
pub async fn kube_rollout_restart(ns: &str, deployment: &str) -> Result<()> {
    let client = get_client().await?;
    let api: Api<Deployment> = Api::namespaced(client.clone(), ns);

    let now = chrono::Utc::now().to_rfc3339();
    let patch = serde_json::json!({
        "spec": {
            "template": {
                "metadata": {
                    "annotations": {
                        "kubectl.kubernetes.io/restartedAt": now
                    }
                }
            }
        }
    });

    api.patch(deployment, &PatchParams::default(), &Patch::Strategic(patch))
        .await
        .with_ctx(|| format!("Failed to restart deployment {ns}/{deployment}"))?;
    Ok(())
}

/// Discover the active domain from cluster state.
///
/// Tries the gitea-inline-config secret first (DOMAIN=src.<domain>),
/// falls back to lasuite-oidc-provider configmap, then Lima VM IP.
#[allow(dead_code)]
pub async fn get_domain() -> Result<String> {
    // 1. Gitea inline-config secret
    if let Ok(Some(secret)) = kube_get_secret("devtools", "gitea-inline-config").await {
        if let Some(data) = &secret.data {
            if let Some(server_bytes) = data.get("server") {
                let server_ini = String::from_utf8_lossy(&server_bytes.0);
                for line in server_ini.lines() {
                    if let Some(rest) = line.strip_prefix("DOMAIN=src.") {
                        return Ok(rest.trim().to_string());
                    }
                }
            }
        }
    }

    // 2. Fallback: lasuite-oidc-provider configmap
    {
        let client = get_client().await?;
        let api: Api<k8s_openapi::api::core::v1::ConfigMap> =
            Api::namespaced(client.clone(), "lasuite");
        if let Ok(Some(cm)) = api.get_opt("lasuite-oidc-provider").await {
            if let Some(data) = &cm.data {
                if let Some(endpoint) = data.get("OIDC_OP_JWKS_ENDPOINT") {
                    if let Some(rest) = endpoint.split("https://auth.").nth(1) {
                        if let Some(domain) = rest.split('/').next() {
                            return Ok(domain.to_string());
                        }
                    }
                }
            }
        }
    }

    // 3. Local dev fallback: Lima VM IP
    let ip = get_lima_ip().await;
    Ok(format!("{ip}.sslip.io"))
}

/// Get the socket_vmnet IP of the Lima sunbeam VM.
async fn get_lima_ip() -> String {
    let output = tokio::process::Command::new("limactl")
        .args(["shell", "sunbeam", "ip", "-4", "addr", "show", "eth1"])
        .output()
        .await;

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if line.contains("inet ") {
                if let Some(addr) = line.trim().split_whitespace().nth(1) {
                    if let Some(ip) = addr.split('/').next() {
                        return ip.to_string();
                    }
                }
            }
        }
    }

    // Fallback: hostname -I
    let output2 = tokio::process::Command::new("limactl")
        .args(["shell", "sunbeam", "hostname", "-I"])
        .output()
        .await;

    if let Ok(out) = output2 {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let ips: Vec<&str> = stdout.trim().split_whitespace().collect();
        if ips.len() >= 2 {
            return ips[ips.len() - 1].to_string();
        } else if !ips.is_empty() {
            return ips[0].to_string();
        }
    }

    String::new()
}

// ---------------------------------------------------------------------------
// kustomize build
// ---------------------------------------------------------------------------

/// Run kustomize build --enable-helm and apply domain/email substitution.
#[allow(dead_code)]
pub async fn kustomize_build(overlay: &Path, domain: &str, email: &str) -> Result<String> {
    let kustomize_path = crate::tools::ensure_kustomize()?;
    let helm_path = crate::tools::ensure_helm()?;

    // Ensure helm's parent dir is on PATH so kustomize can find it
    let helm_dir = helm_path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut env_path = helm_dir.clone();
    if let Ok(existing) = std::env::var("PATH") {
        env_path = format!("{helm_dir}:{existing}");
    }

    let output = tokio::process::Command::new(&kustomize_path)
        .args(["build", "--enable-helm"])
        .arg(overlay)
        .env("PATH", &env_path)
        .output()
        .await
        .ctx("Failed to run kustomize")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("kustomize build failed: {stderr}");
    }

    let mut text = String::from_utf8(output.stdout).ctx("kustomize output not UTF-8")?;

    // Domain substitution
    text = domain_replace(&text, domain);

    // ACME email substitution
    if !email.is_empty() {
        text = text.replace("ACME_EMAIL", email);
    }

    // Registry host IP resolution
    if text.contains("REGISTRY_HOST_IP") {
        let registry_ip = resolve_registry_ip(domain).await;
        text = text.replace("REGISTRY_HOST_IP", &registry_ip);
    }

    // Strip null annotations artifact
    text = text.replace("\n      annotations: null", "");

    Ok(text)
}

/// Resolve the registry host IP for REGISTRY_HOST_IP substitution.
async fn resolve_registry_ip(domain: &str) -> String {
    // Try DNS for src.<domain>
    let hostname = format!("src.{domain}:443");
    if let Ok(mut addrs) = tokio::net::lookup_host(&hostname).await {
        if let Some(addr) = addrs.next() {
            return addr.ip().to_string();
        }
    }

    // Fallback: derive from production host config
    let ssh_host = crate::config::get_production_host();
    if !ssh_host.is_empty() {
        let raw = ssh_host
            .split('@')
            .last()
            .unwrap_or(&ssh_host)
            .split(':')
            .next()
            .unwrap_or(&ssh_host);
        let host_lookup = format!("{raw}:443");
        if let Ok(mut addrs) = tokio::net::lookup_host(&host_lookup).await {
            if let Some(addr) = addrs.next() {
                return addr.ip().to_string();
            }
        }
        // raw is likely already an IP
        return raw.to_string();
    }

    String::new()
}

// ---------------------------------------------------------------------------
// kubectl / bao passthrough
// ---------------------------------------------------------------------------

/// Transparent kubectl passthrough for the active context.
pub async fn cmd_k8s(kubectl_args: &[String]) -> Result<()> {
    ensure_tunnel().await?;

    let status = tokio::process::Command::new("kubectl")
        .arg(format!("--context={}", context()))
        .args(kubectl_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .ctx("Failed to run kubectl")?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Run bao CLI inside the OpenBao pod with the root token.
pub async fn cmd_bao(bao_args: &[String]) -> Result<()> {
    // Find the openbao pod
    let client = get_client().await?;
    let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client.clone(), "data");

    let lp = ListParams::default().labels("app.kubernetes.io/name=openbao");
    let pod_list = pods.list(&lp).await.ctx("Failed to list OpenBao pods")?;
    let ob_pod = pod_list
        .items
        .first()
        .and_then(|p| p.metadata.name.as_deref())
        .ctx("OpenBao pod not found -- is the cluster running?")?
        .to_string();

    // Get root token
    let root_token = kube_get_secret_field("data", "openbao-keys", "root-token")
        .await
        .ctx("root-token not found in openbao-keys secret")?;

    // Build the exec command using env to set VAULT_TOKEN without shell interpretation
    let vault_token_env = format!("VAULT_TOKEN={root_token}");
    let mut kubectl_args = vec![
        format!("--context={}", context()),
        "-n".to_string(),
        "data".to_string(),
        "exec".to_string(),
        ob_pod,
        "-c".to_string(),
        "openbao".to_string(),
        "--".to_string(),
        "env".to_string(),
        vault_token_env,
        "bao".to_string(),
    ];
    kubectl_args.extend(bao_args.iter().cloned());

    // Use kubectl for full TTY support
    let status = tokio::process::Command::new("kubectl")
        .args(&kubectl_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .ctx("Failed to run bao in OpenBao pod")?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parse target and domain_replace (already tested)
// ---------------------------------------------------------------------------

/// Parse 'ns/name' -> (Some(ns), Some(name)), 'ns' -> (Some(ns), None), None -> (None, None).
pub fn parse_target(s: Option<&str>) -> Result<(Option<&str>, Option<&str>)> {
    match s {
        None => Ok((None, None)),
        Some(s) => {
            let parts: Vec<&str> = s.splitn(3, '/').collect();
            match parts.len() {
                1 => Ok((Some(parts[0]), None)),
                2 => Ok((Some(parts[0]), Some(parts[1]))),
                _ => bail!("Invalid target {s:?}: expected 'namespace' or 'namespace/name'"),
            }
        }
    }
}

/// Replace all occurrences of DOMAIN_SUFFIX with domain.
pub fn domain_replace(text: &str, domain: &str) -> String {
    text.replace("DOMAIN_SUFFIX", domain)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_target_none() {
        let (ns, name) = parse_target(None).unwrap();
        assert!(ns.is_none());
        assert!(name.is_none());
    }

    #[test]
    fn test_parse_target_namespace_only() {
        let (ns, name) = parse_target(Some("ory")).unwrap();
        assert_eq!(ns, Some("ory"));
        assert!(name.is_none());
    }

    #[test]
    fn test_parse_target_namespace_and_name() {
        let (ns, name) = parse_target(Some("ory/kratos")).unwrap();
        assert_eq!(ns, Some("ory"));
        assert_eq!(name, Some("kratos"));
    }

    #[test]
    fn test_parse_target_too_many_parts() {
        assert!(parse_target(Some("too/many/parts")).is_err());
    }

    #[test]
    fn test_parse_target_empty_string() {
        let (ns, name) = parse_target(Some("")).unwrap();
        assert_eq!(ns, Some(""));
        assert!(name.is_none());
    }

    #[test]
    fn test_domain_replace_single() {
        let result = domain_replace("src.DOMAIN_SUFFIX/foo", "192.168.1.1.sslip.io");
        assert_eq!(result, "src.192.168.1.1.sslip.io/foo");
    }

    #[test]
    fn test_domain_replace_multiple() {
        let result = domain_replace("DOMAIN_SUFFIX and DOMAIN_SUFFIX", "x.sslip.io");
        assert_eq!(result, "x.sslip.io and x.sslip.io");
    }

    #[test]
    fn test_domain_replace_none() {
        let result = domain_replace("no match here", "x.sslip.io");
        assert_eq!(result, "no match here");
    }

    #[tokio::test]
    async fn test_ensure_tunnel_noop_when_ssh_host_empty() {
        // When ssh_host is empty (local dev), ensure_tunnel should return Ok
        // immediately without spawning any SSH process.
        // SSH_HOST OnceLock may already be set from another test, but the
        // default (unset) value is "" which is what we want. If it was set
        // to a non-empty value by a prior test in the same process, this
        // test would attempt a real SSH connection and fail — that is acceptable
        // as a signal that test isolation changed.
        //
        // In a fresh test binary SSH_HOST is unset, so ssh_host() returns "".
        let result = ensure_tunnel().await;
        assert!(result.is_ok(), "ensure_tunnel should be a no-op when ssh_host is empty");
    }

    #[test]
    fn test_create_secret_data_encoding() {
        // Test that we can build the expected JSON structure for secret creation
        let mut data = HashMap::new();
        data.insert("username".to_string(), "admin".to_string());
        data.insert("password".to_string(), "s3cret".to_string());

        let mut encoded: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (k, v) in &data {
            let b64 = base64::engine::general_purpose::STANDARD.encode(v.as_bytes());
            encoded.insert(k.clone(), serde_json::Value::String(b64));
        }

        let secret_obj = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "test-secret",
                "namespace": "default",
            },
            "type": "Opaque",
            "data": encoded,
        });

        let json_str = serde_json::to_string(&secret_obj).unwrap();
        assert!(json_str.contains("YWRtaW4=")); // base64("admin")
        assert!(json_str.contains("czNjcmV0")); // base64("s3cret")
    }
}
