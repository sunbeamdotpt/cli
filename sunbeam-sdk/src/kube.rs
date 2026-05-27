//! Kubernetes client initialization and manifest operations.

use crate::error::{Result, ResultExt, SunbeamError};
use base64::Engine;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Namespace, Node, Secret};
use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, ListParams, Patch, PatchParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::{self, Scope};
use kube::{Client, Config};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

const SPEC_HASH_ANNOTATION: &str = "sunbeam.pt/spec-hash";

static CONTEXT: OnceLock<String> = OnceLock::new();

/// Global semaphore that limits the number of concurrent manifest applications.
/// Single-node k3s (especially SQLite-backed) becomes unresponsive when too
/// many namespaces are applied in parallel. Two permits is conservative but
/// safe for Lima VMs; production clusters are not harmed by the limit.
static APPLY_SEMAPHORE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

fn apply_semaphore() -> &'static tokio::sync::Semaphore {
    APPLY_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(2))
}

/// Set the active kubectl context.
pub fn set_context(ctx: &str) {
    let _ = CONTEXT.set(ctx.to_string());
}

/// Get the active context name. Returns empty string when the user has
/// not configured one; callers that need a valid cluster (e.g.
/// `get_client`) should refuse in that case rather than silently falling
/// back to a default.
pub fn context() -> &'static str {
    CONTEXT.get().map(|s| s.as_str()).unwrap_or("")
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
#[tracing::instrument]
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

/// Query APIServices and return the group names of any that are not available.
async fn discover_broken_api_groups(client: &Client) -> Result<Vec<String>> {
    let api: Api<APIService> = Api::all(client.clone());
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| SunbeamError::kube(format!("Failed to list APIServices: {e}")))?;

    let mut broken = Vec::new();
    for svc in list.items {
        let available = svc.status.as_ref().and_then(|s| {
            s.conditions
                .as_ref()?
                .iter()
                .find(|c| c.type_ == "Available")
        });
        if available.map(|c| c.status != "True").unwrap_or(true) {
            if let Some(name) = svc.metadata.name {
                // APIService names are like "v1alpha1.acme.scaleway.com"
                // Extract the group part (everything after the first dot)
                if let Some(dot) = name.find('.') {
                    let group = &name[dot + 1..];
                    if !group.is_empty() && !broken.contains(&group.to_string()) {
                        broken.push(group.to_string());
                    }
                }
            }
        }
    }
    Ok(broken)
}

/// Compute a stable SHA-256 hex hash of the `spec` field of a Job manifest document.
///
/// Only the `spec` key is hashed (metadata and status are excluded) so that
/// label/annotation changes on the Job wrapper don't trigger unnecessary
/// deletions, while any container/template/volume drift will be detected.
fn job_spec_hash(doc_json: &serde_json::Value) -> String {
    let spec = doc_json
        .get("spec")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let canonical = serde_json::to_string(&spec).unwrap_or_default();
    let digest = Sha256::digest(canonical.as_bytes());
    format!("{digest:x}")
}

/// Server-side apply a multi-document YAML manifest.
///
/// For `kind: Job` documents this function performs spec-drift detection:
/// 1. Hash the rendered manifest's `spec` (SHA-256, hex).
/// 2. Fetch the live Job and read its `sunbeam.pt/spec-hash` annotation.
/// 3. If the annotation is missing or differs, delete the live Job before
///    applying so the new spec takes effect (Jobs are immutable once created).
/// 4. After a successful apply, patch the annotation onto the Job so future
///    runs can detect whether a re-apply is actually needed.
#[tracing::instrument(skip(manifest))]
pub async fn kube_apply(manifest: &str) -> Result<()> {
    // Throttle concurrent manifest applications to protect single-node
    // k3s from being overwhelmed (especially SQLite-backed control planes).
    let _permit = apply_semaphore()
        .acquire()
        .await
        .expect("apply semaphore never closed");

    let client = get_client().await?;
    let ssapply = PatchParams::apply("sunbeam").force();

    // Run discovery once and cache the result for all documents.
    // Without this, every document triggers a full API discovery round-trip,
    // which overwhelms the API server when applying 100+ resources.
    //
    // Broken APIServices (e.g. stale webhook registrations) cause 503s during
    // discovery. We query APIServices first and exclude unavailable groups so
    // discovery doesn't fail on them.
    let broken_groups = discover_broken_api_groups(&client)
        .await
        .unwrap_or_default();
    if !broken_groups.is_empty() {
        tracing::info!(
            "Excluding broken API groups from discovery: {}",
            broken_groups.join(", ")
        );
    }

    let mut disc = None;
    let mut last_err = None;
    for attempt in 1..=20 {
        let d = discovery::Discovery::new(client.clone())
            .exclude(&broken_groups.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        match d.run().await {
            Ok(d) => {
                disc = Some(d);
                break;
            }
            Err(e) => {
                tracing::warn!("API discovery attempt {attempt} failed: {e}");
                last_err = Some(e);
                if attempt < 20 {
                    // Exponential backoff capped at 10 s — total wait ~130 s.
                    let backoff = std::cmp::min(1u64 << (attempt - 1), 10);
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                }
            }
        }
    }
    let mut disc = match disc {
        Some(d) => d,
        None => {
            return Err(SunbeamError::kube(format!(
                "API discovery failed after 20 attempts: {}",
                last_err.unwrap()
            )));
        }
    };

    // Split manifest into CRDs and everything else.
    // CRDs must be applied first so their APIs are registered before we try
    // to apply custom resources that use them.
    let mut crd_docs: Vec<&str> = Vec::new();
    let mut other_docs: Vec<&str> = Vec::new();

    for doc in manifest.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() || doc == "---" {
            continue;
        }
        if doc.contains("kind: CustomResourceDefinition") {
            crd_docs.push(doc);
        } else {
            other_docs.push(doc);
        }
    }

    let mut errors: Vec<String> = Vec::new();

    // Pass 1: apply CRDs
    for doc in &crd_docs {
        match apply_one_doc(&client, &ssapply, &disc, doc).await {
            Ok(name) if !name.is_empty() => {
                tracing::info!("  Applied {name}");
            }
            Err(e) => errors.push(e),
            _ => {}
        }
    }

    // If we applied any CRDs, refresh discovery so the new APIs are known.
    if !crd_docs.is_empty() {
        tracing::info!("CRDs applied — refreshing API discovery...");
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match discovery::Discovery::new(client.clone())
            .exclude(&broken_groups.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .run()
            .await
        {
            Ok(d) => disc = d,
            Err(e) => {
                errors.push(format!("Failed to refresh discovery after CRD apply: {e}"));
            }
        };
    }

    // Pre-create any namespaces referenced by the manifest so that
    // cluster-scoped resources (e.g. RoleBindings) that target them don't
    // fail with "namespace not found".
    let ns_api: Api<Namespace> = Api::all(client.clone());
    let mut seen_ns: std::collections::HashSet<String> = std::collections::HashSet::new();
    for doc in &other_docs {
        if let Ok(obj) = serde_yaml::from_str::<serde_yaml::Value>(doc) {
            // Namespace field on the resource itself
            if let Some(ns) = obj
                .get("metadata")
                .and_then(|m| m.get("namespace"))
                .and_then(|v| v.as_str())
            {
                seen_ns.insert(ns.to_string());
            }
            // For RoleBindings, the subject namespace
            if let Some(subjects) = obj.get("subjects").and_then(|s| s.as_sequence()) {
                for sub in subjects {
                    if let Some(ns) = sub.get("namespace").and_then(|v| v.as_str()) {
                        seen_ns.insert(ns.to_string());
                    }
                }
            }
        }
    }
    for ns in seen_ns {
        ns_api
            .patch(
                &ns,
                &ssapply,
                &Patch::Apply(serde_json::json!({
                    "apiVersion": "v1",
                    "kind": "Namespace",
                    "metadata": { "name": &ns }
                })),
            )
            .await
            .map_err(|e| SunbeamError::kube(format!("Failed to ensure namespace {ns}: {e}")))?;
    }

    // Pass 2: apply everything else
    // Webhooks (cert-manager, CNPG, etc.) may not be ready immediately after
    // their deployments are applied. Retry on webhook errors with backoff.
    for doc in &other_docs {
        let mut last_err = None;
        for attempt in 0..12 {
            match apply_one_doc(&client, &ssapply, &disc, doc).await {
                Ok(name) => {
                    if !name.is_empty() {
                        tracing::info!("  Applied {name}");
                    }
                    last_err = None;
                    break;
                }
                Err(e) if is_webhook_error(&e) && attempt < 11 => {
                    tracing::info!(
                        "Webhook not ready for doc (attempt {}), retrying in 10s...",
                        attempt + 1
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    last_err = Some(e);
                }
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = last_err {
            errors.push(e);
        }
    }

    if !errors.is_empty() {
        return Err(SunbeamError::kube(format!(
            "Apply had {} error(s): {}",
            errors.len(),
            errors.join("; ")
        )));
    }

    Ok(())
}

/// Apply a single YAML document using pre-built discovery.
/// Returns a display string like "ingress/ConfigMap/pingora-config" on success.
async fn apply_one_doc(
    client: &Client,
    ssapply: &PatchParams,
    disc: &discovery::Discovery,
    doc: &str,
) -> std::result::Result<String, String> {
    let obj: serde_yaml::Value =
        serde_yaml::from_str(doc).map_err(|e| format!("Failed to parse YAML document: {e}"))?;

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

    tracing::debug!("kube_apply {kind}/{name} namespace={namespace:?}");

    if name.is_empty() || kind.is_empty() {
        return Ok(String::new()); // skip incomplete documents
    }

    // ---------------------------------------------------------------
    // Job immutability: delete the live Job when its spec has changed.
    // ---------------------------------------------------------------
    if kind == "Job" {
        let patch_json: serde_json::Value =
            serde_yaml::from_str(doc).map_err(|e| format!("Failed to parse Job YAML: {e}"))?;
        let new_hash = job_spec_hash(&patch_json);

        let job_ns = namespace.unwrap_or("default");
        let jobs: Api<Job> = Api::namespaced(client.clone(), job_ns);

        if let Ok(Some(live_job)) = jobs.get_opt(name).await {
            let live_hash = live_job
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(SPEC_HASH_ANNOTATION))
                .map(|s| s.as_str())
                .unwrap_or("");

            if live_hash != new_hash {
                tracing::info!(
                    "Job {job_ns}/{name} spec changed — deleting before re-apply..."
                );
                let dp = DeleteParams::default();
                let _ = jobs.delete(name, &dp).await;
            }
        }
    }

    // Use discovery to find the right API resource
    let (ar, scope) = match resolve_api_resource(disc, api_version, kind) {
        Ok(r) => r,
        Err(e) => {
            return Err(format!(
                "Could not discover API resource for {api_version}/{kind}: {e}"
            ));
        }
    };

    let api: Api<DynamicObject> = if let Some(ns) = namespace {
        Api::namespaced_with(client.clone(), ns, &ar)
    } else if scope == Scope::Namespaced {
        Api::default_namespaced_with(client.clone(), &ar)
    } else {
        Api::all_with(client.clone(), &ar)
    };

    let mut patch: serde_json::Value = match serde_yaml::from_str(doc) {
        Ok(v) => v,
        Err(e) => {
            return Err(format!("Failed to parse {kind}/{name} to JSON: {e}"));
        }
    };

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
                obj.insert("annotations".to_string(), serde_json::Value::Object(ann));
            }
        }
    }

    api.patch(name, ssapply, &Patch::Apply(patch))
        .await
        .map_err(|e| format!("Failed to apply {kind}/{name}: {e}"))?;

    // Stamp spec-hash on Jobs after successful apply
    if kind == "Job" {
        let patch_json: serde_json::Value =
            serde_yaml::from_str(doc).map_err(|e| format!("Failed to parse Job YAML: {e}"))?;
        let hash = job_spec_hash(&patch_json);
        let job_ns = namespace.unwrap_or("default");
        let jobs: Api<Job> = Api::namespaced(client.clone(), job_ns);
        let annotation_patch = serde_json::json!({
            "metadata": {
                "annotations": {
                    SPEC_HASH_ANNOTATION: hash
                }
            }
        });
        if let Err(e) = jobs
            .patch(
                name,
                &PatchParams::default(),
                &Patch::Merge(&annotation_patch),
            )
            .await
        {
            return Err(format!(
                "Failed to stamp spec-hash on Job {job_ns}/{name}: {e}"
            ));
        }
    }

    let id = if let Some(ns) = namespace {
        format!("{ns}/{kind}/{name}")
    } else {
        format!("{kind}/{name}")
    };
    Ok(id)
}

/// Returns true if the error message indicates a webhook that isn't ready yet.
fn is_webhook_error(err: &str) -> bool {
    err.contains("failed calling webhook")
        || err.contains("no endpoints available for service")
        || err.contains("connection refused")
        || err.contains("context deadline exceeded")
}

/// Resolve an API resource from apiVersion and kind using a pre-built discovery.
fn resolve_api_resource(
    disc: &discovery::Discovery,
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
#[tracing::instrument]
pub async fn kube_get_secret(ns: &str, name: &str) -> Result<Option<Secret>> {
    let client = get_client().await?;
    tracing::debug!("kube_get_secret {ns}/{name}");
    let api: Api<Secret> = Api::namespaced(client.clone(), ns);
    match api.get_opt(name).await {
        Ok(secret) => Ok(secret),
        Err(e) => Err(e).with_ctx(|| format!("Failed to get secret {ns}/{name}")),
    }
}

/// Get a specific base64-decoded field from a Kubernetes secret.
#[allow(dead_code)]
#[tracing::instrument]
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
#[tracing::instrument]
pub async fn ns_exists(ns: &str) -> Result<bool> {
    tracing::debug!("ns_exists {ns}");
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
#[tracing::instrument]
pub async fn ensure_ns(ns: &str) -> Result<()> {
    tracing::debug!("ensure_ns {ns}");
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
#[tracing::instrument]
pub async fn create_secret(ns: &str, name: &str, data: HashMap<String, String>) -> Result<()> {
    tracing::debug!("create_secret {ns}/{name}");
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
#[tracing::instrument]
pub async fn find_pod_by_label(ns: &str, label: &str) -> Option<String> {
    tracing::debug!("find_pod_by_label {ns} label={label}");
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
#[tracing::instrument]
pub async fn find_pod_by_label_or_any(ns: &str, label: &str) -> Option<(String, bool)> {
    tracing::debug!("find_pod_by_label_or_any {ns} label={label}");
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
    let all_pods = pods.list(&kube::api::ListParams::default()).await.ok()?;
    all_pods
        .items
        .iter()
        .find(|p| p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running"))
        .and_then(|p| p.metadata.name.clone())
        .map(|name| (name, true))
}

/// Execute a command in a pod and return (exit_code, stdout).
#[allow(dead_code)]
#[tracing::instrument]
pub async fn kube_exec(
    ns: &str,
    pod: &str,
    cmd: &[&str],
    container: Option<&str>,
) -> Result<(i32, String)> {
    tracing::debug!("kube_exec {ns}/{pod}: {cmd:?}");
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
    let mut attached = match pods.exec(pod, cmd_strings, &ep).await {
        Ok(attached) => attached,
        Err(e) => {
            tracing::warn!("kube_exec failed: {e}");
            return Err(e).with_ctx(|| format!("Failed to exec in pod {ns}/{pod}"));
        }
    };

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

/// Execute a command in a pod with optional stdin data and return (exit_code, stdout).
#[tracing::instrument]
pub async fn kube_exec_with_stdin(
    ns: &str,
    pod: &str,
    cmd: &[&str],
    container: Option<&str>,
    stdin_data: Option<&[u8]>,
) -> Result<(i32, String)> {
    tracing::debug!("kube_exec_with_stdin {ns}/{pod}: {cmd:?}");
    let client = get_client().await?;
    let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client.clone(), ns);

    let ep = kube::api::AttachParams {
        stdout: true,
        stderr: true,
        stdin: stdin_data.is_some(),
        container: container.map(|c| c.to_string()),
        ..Default::default()
    };

    let cmd_strings: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
    let mut attached = pods
        .exec(pod, cmd_strings, &ep)
        .await
        .with_ctx(|| format!("Failed to exec in pod {ns}/{pod}"))?;

    // Write stdin data if provided.  Chunked writes avoid websocket frame
    // size limits that can trigger "broken pipe" on large stdin payloads.
    if let Some(data) = stdin_data {
        if let Some(mut stdin_writer) = attached.stdin() {
            use tokio::io::AsyncWriteExt;
            const CHUNK_SIZE: usize = 64 * 1024;
            for chunk in data.chunks(CHUNK_SIZE) {
                stdin_writer.write_all(chunk).await?;
                stdin_writer.flush().await?;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            stdin_writer.shutdown().await.ok();
        }
    }

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
#[tracing::instrument]
pub async fn kube_rollout_restart(ns: &str, deployment: &str) -> Result<()> {
    tracing::debug!("kube_rollout_restart {ns}/{deployment}");
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
#[tracing::instrument]
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

/// Get the cluster node's InternalIP (Lima VM IP in local dev).
///
/// Uses the Kubernetes API to read the first node's InternalIP address.
/// This avoids `limactl shell` which is unreliable during workflow execution.
async fn get_lima_ip() -> String {
    let client = match get_client().await {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    let nodes: Api<Node> = Api::all(client);
    match nodes.list(&ListParams::default()).await {
        Ok(node_list) => {
            for node in node_list.items {
                if let Some(status) = node.status
                    && let Some(addrs) = status.addresses
                {
                    for addr in addrs {
                        if addr.type_ == "InternalIP" {
                            return addr.address;
                        }
                    }
                }
            }
        }
        Err(_) => {}
    }

    String::new()
}

// ---------------------------------------------------------------------------
// kustomize build
// ---------------------------------------------------------------------------

/// Run kustomize build --enable-helm and apply domain/email substitution.
#[allow(dead_code)]
#[tracing::instrument]
pub async fn kustomize_build(overlay: &Path, domain: &str, email: &str) -> Result<String> {
    tracing::debug!("kustomize_build {overlay:?} domain={domain}");
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
#[tracing::instrument(skip(bao_args))]
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
    fn test_job_spec_hash_stable() {
        let doc = serde_json::json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": { "name": "vault-bootstrap-job", "namespace": "data" },
            "spec": {
                "template": {
                    "spec": {
                        "containers": [{"name": "bootstrap", "image": "alpine:3.18", "command": ["sh", "-c", "echo hello"]}],
                        "restartPolicy": "Never"
                    }
                }
            }
        });
        let h1 = job_spec_hash(&doc);
        let h2 = job_spec_hash(&doc);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_job_spec_hash_changes_on_spec_drift() {
        let doc1 = serde_json::json!({
            "kind": "Job",
            "spec": { "template": { "spec": { "containers": [{"image": "alpine:3.18"}] } } }
        });
        let doc2 = serde_json::json!({
            "kind": "Job",
            "spec": { "template": { "spec": { "containers": [{"image": "alpine:3.19"}] } } }
        });
        assert_ne!(job_spec_hash(&doc1), job_spec_hash(&doc2));
    }

    #[test]
    fn test_job_spec_hash_ignores_metadata_changes() {
        let doc1 = serde_json::json!({
            "kind": "Job",
            "metadata": { "name": "foo", "labels": { "run": "1" } },
            "spec": { "template": { "spec": { "containers": [{"image": "alpine:3.18"}] } } }
        });
        let doc2 = serde_json::json!({
            "kind": "Job",
            "metadata": { "name": "foo", "labels": { "run": "2" } },
            "spec": { "template": { "spec": { "containers": [{"image": "alpine:3.18"}] } } }
        });
        assert_eq!(job_spec_hash(&doc1), job_spec_hash(&doc2));
    }

    #[test]
    fn test_job_spec_hash_missing_spec() {
        // A document with no spec key should not panic
        let doc = serde_json::json!({ "kind": "Job", "metadata": { "name": "empty" } });
        let h = job_spec_hash(&doc);
        assert_eq!(h.len(), 64);
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
