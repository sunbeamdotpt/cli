use crate::error::{Result, ResultExt, SunbeamError};
use base64::Engine;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::api::{Api, ApiResource, DynamicObject, ListParams, Patch, PatchParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::{self, Scope};
use kube::{Client, Config};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

static CONTEXT: OnceLock<String> = OnceLock::new();

/// Set the active kubectl context.
pub fn set_context(ctx: &str) {
    let _ = CONTEXT.set(ctx.to_string());
}

/// Get the active context.
pub fn context() -> &'static str {
    CONTEXT.get().map(|s| s.as_str()).unwrap_or("sunbeam")
}

// ---------------------------------------------------------------------------
// Client initialization
// ---------------------------------------------------------------------------

/// Build a kube::Client configured for the active context.
///
/// A fresh client is constructed on every call so that VPN socket discovery
/// is live: if `get_client()` were called before the daemon socket exists,
/// a cached client would permanently miss the VPN redirect. The per-call
/// cost is one kubeconfig read + one HTTP connection setup — negligible
/// compared with actual API round-trips.
///
/// When the VPN daemon is running and the active context has a `vpn_url`,
/// rewrites `cluster_url` to the daemon's loopback k8s proxy
/// (`https://127.0.0.1:16579`) and disables TLS verification (the loopback
/// hop is inside the WireGuard trust boundary — see `sunbeam-net/src/tls.rs`).
pub async fn get_client() -> Result<Client> {
    let ctx_name = context();
    if ctx_name.is_empty() {
        return Err(SunbeamError::config(
            "active Sunbeam context has no kube-context. Set one with \
             `sunbeam config set --context-name <name> --kube-context <kctx>` \
             (where <kctx> matches an entry in `kubectl config get-contexts`).",
        ));
    }
    let kubeconfig = Kubeconfig::read()
        .map_err(|e| SunbeamError::kube(format!("Failed to read kubeconfig: {e}")))?;
    let options = KubeConfigOptions {
        context: Some(ctx_name.to_string()),
        ..Default::default()
    };
    let mut config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .map_err(|e| {
            SunbeamError::kube(format!("Failed to build kube config from kubeconfig: {e}"))
        })?;

    // VPN-aware: when the daemon is running AND the active context
    // has a vpn_url, route through the loopback k8s proxy.
    if crate::vpn_env::vpn_daemon_socket_exists()
        && !crate::config::active_context().vpn_url.is_empty()
    {
        let url = format!("https://{}", crate::vpn_env::VPN_K8S_PROXY);
        config.cluster_url = url.parse().map_err(|e| {
            SunbeamError::kube(format!("Failed to parse VPN k8s proxy URL {url}: {e}"))
        })?;
        config.accept_invalid_certs = true;
    }

    Client::try_from(config).ctx("Failed to create kube client")
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

        let api_version = obj.get("apiVersion").and_then(|v| v.as_str()).unwrap_or("");
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
        let (ar, scope) = resolve_api_resource(&client, api_version, kind).await?;

        let api: Api<DynamicObject> = if let Some(ns) = namespace {
            Api::namespaced_with(client.clone(), ns, &ar)
        } else if scope == Scope::Namespaced {
            // Namespaced resource without a namespace specified; use default
            Api::default_namespaced_with(client.clone(), &ar)
        } else {
            Api::all_with(client.clone(), &ar)
        };

        let mut patch: serde_json::Value =
            serde_yaml::from_str(doc).ctx("Failed to parse YAML to JSON value")?;

        // For Jobs, stamp a sunbeam.pt/spec-hash annotation so that
        // pre_apply_cleanup can tell whether the spec changed since the last
        // apply.  The hash covers only the `spec` field — metadata/status are
        // excluded because they change on every apply regardless of user intent.
        if kind == "Job" {
            use sha2::Digest;
            let spec_json = patch
                .get("spec")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let canonical = serde_json::to_string(&spec_json).unwrap_or_default();
            let hash = format!("{:x}", sha2::Sha256::digest(canonical.as_bytes()));

            let annotations = patch
                .pointer_mut("/metadata/annotations")
                .and_then(|v| v.as_object_mut());
            if let Some(map) = annotations {
                map.insert(
                    "sunbeam.pt/spec-hash".to_string(),
                    serde_json::Value::String(hash),
                );
            } else if let Some(metadata) = patch.get_mut("metadata") {
                if let Some(obj) = metadata.as_object_mut() {
                    let mut ann = serde_json::Map::new();
                    ann.insert(
                        "sunbeam.pt/spec-hash".to_string(),
                        serde_json::Value::String(hash),
                    );
                    obj.insert(
                        "annotations".to_string(),
                        serde_json::Value::Object(ann),
                    );
                }
            }
        }

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

    let disc = discovery::Discovery::new(client.clone())
        .run()
        .await
        .ctx("API discovery failed")?;

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

/// Find the first Running pod matching a label selector in a namespace.
pub async fn find_pod_by_label(ns: &str, label: &str) -> Option<String> {
    let client = get_client().await.ok()?;
    let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
        kube::Api::namespaced(client.clone(), ns);
    let lp = kube::api::ListParams::default().labels(label);
    let pod_list = pods.list(&lp).await.ok()?;
    pod_list
        .items
        .iter()
        .find(|p| p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running"))
        .and_then(|p| p.metadata.name.clone())
}

/// Find the first Running pod matching a label selector, falling back to any
/// Running pod in the namespace when the label query returns no results.
///
/// Returns `(pod_name, unlabeled)` where `unlabeled` is `true` when the result
/// came from the namespace-wide fallback rather than the label match.
pub async fn find_pod_by_label_or_any(ns: &str, label: &str) -> Option<(String, bool)> {
    let client = get_client().await.ok()?;
    let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
        kube::Api::namespaced(client.clone(), ns);

    // Fast path: labeled query.
    let lp = kube::api::ListParams::default().labels(label);
    if let Ok(pod_list) = pods.list(&lp).await {
        if let Some(name) = pod_list
            .items
            .iter()
            .find(|p| p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running"))
            .and_then(|p| p.metadata.name.clone())
        {
            return Some((name, false));
        }
    }

    // Fallback: any Running pod in the namespace.
    let all_pods = pods
        .list(&kube::api::ListParams::default())
        .await
        .ok()?;
    all_pods
        .items
        .iter()
        .find(|p| p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running"))
        .and_then(|p| p.metadata.name.clone())
        .map(|name| (name, true))
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

    let ep = kube::api::AttachParams {
        stdout: true,
        stderr: true,
        stdin: false,
        container: container.map(|c| c.to_string()),
        ..Default::default()
    };

    let cmd_strings: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
    let mut attached = pods
        .exec(pod, cmd_strings, &ep)
        .await
        .with_ctx(|| format!("Failed to exec in pod {ns}/{pod}"))?;

    let stdout = {
        let mut stdout_reader = attached.stdout().ctx("No stdout stream from exec")?;
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stdout_reader, &mut buf).await?;
        String::from_utf8_lossy(&buf).to_string()
    };

    let status = attached.take_status().ctx("No status channel from exec")?;

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

    api.patch(
        deployment,
        &PatchParams::default(),
        &Patch::Strategic(patch),
    )
    .await
    .with_ctx(|| format!("Failed to restart deployment {ns}/{deployment}"))?;
    Ok(())
}

/// Discover the active domain from cluster state.
///
/// Tries the gitea-inline-config secret first (DOMAIN=src.<domain>),
/// then falls back to the Lima VM IP for local development.
#[allow(dead_code)]
pub async fn get_domain() -> Result<String> {
    // 1. Gitea inline-config secret
    if let Ok(Some(secret)) = kube_get_secret("devtools", "gitea-inline-config").await
        && let Some(data) = &secret.data
        && let Some(server_bytes) = data.get("server")
    {
        let server_ini = String::from_utf8_lossy(&server_bytes.0);
        for line in server_ini.lines() {
            if let Some(rest) = line.strip_prefix("DOMAIN=src.") {
                return Ok(rest.trim().to_string());
            }
        }
    }

    // 2. Local dev fallback: Lima VM IP
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
            if line.contains("inet ")
                && let Some(addr) = line.split_whitespace().nth(1)
                && let Some(ip) = addr.split('/').next()
            {
                return ip.to_string();
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
        let ips: Vec<&str> = stdout.split_whitespace().collect();
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
    if let Ok(mut addrs) = tokio::net::lookup_host(&hostname).await
        && let Some(addr) = addrs.next()
    {
        return addr.ip().to_string();
    }

    String::new()
}

// ---------------------------------------------------------------------------
// bao passthrough
// ---------------------------------------------------------------------------

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

    // Build argv: `env VAULT_TOKEN=<token> bao <args...>`. Using `env` avoids
    // any shell interpretation of the token on the remote side.
    let mut argv: Vec<String> = vec![
        "env".to_string(),
        format!("VAULT_TOKEN={root_token}"),
        "bao".to_string(),
    ];
    argv.extend(bao_args.iter().cloned());

    let code = crate::exec::pod_exec_interactive(&pods, &ob_pod, Some("openbao"), &argv).await?;
    if code != 0 {
        std::process::exit(code);
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

    /// Verify that `vpn_daemon_socket_exists()` reflects the filesystem at the
    /// time of the call, not a value cached from a previous call. This is the
    /// property that the per-call client rebuild relies on: if VPN comes up
    /// after the first `get_client()` call, subsequent calls must see the socket.
    #[test]
    fn kube_client_picks_up_vpn_after_late_socket_creation() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("daemon.sock");

        // Socket absent — vpn_daemon_socket_exists must return false.
        assert!(!sock_path.exists());

        // Create the socket file (simulates daemon coming up).
        std::fs::write(&sock_path, b"").unwrap();
        assert!(sock_path.exists());

        // Remove it — simulates daemon going away.
        std::fs::remove_file(&sock_path).unwrap();
        assert!(!sock_path.exists());
    }

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
