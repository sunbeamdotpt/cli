//! Kustomize build, apply, and namespace filtering.

use crate::error::{Result, ResultExt};

/// Return only the YAML documents that belong to the given namespace.
///
/// Keeps namespace-scoped resources in the target namespace, the Namespace
/// resource itself, and cluster-scoped resources (which have no `namespace:`
/// field). The latter are needed because Helm charts emit ClusterRoles and
/// CRDs that are required for the namespace's workloads to function.
///
/// `skip_patterns`: if any pattern is found in a document's text, that
/// document is dropped. Used to exclude e.g. the Scaleway DNS webhook on
/// local dev domains.
pub fn filter_by_namespace(manifests: &str, namespace: &str, skip_patterns: &[String]) -> String {
    let mut kept = Vec::new();
    for doc in manifests.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() {
            continue;
        }
        let has_target_ns = doc.contains(&format!("namespace: {namespace}"));
        let is_target_ns =
            doc.contains("kind: Namespace") && doc.contains(&format!("name: {namespace}"));
        // Cluster-scoped resources (ClusterRole, CRD, etc.) don't have a
        // namespace field. Include them so that Helm RBAC and webhooks are
        // not silently dropped. Exclude Namespace resources for other
        // namespaces — those are not cluster-scoped in the logical sense.
        let is_cluster_scoped = !doc.contains("namespace: ") && !doc.contains("kind: Namespace");
        // Some charts (cert-manager) create Roles/RoleBindings in kube-system
        // for leader election. Include those so RBAC isn't silently dropped.
        let is_system_ns = doc.contains("namespace: kube-system");
        let should_skip = skip_patterns.iter().any(|p| {
            // Match on metadata.name to avoid accidentally skipping resources
            // that merely mention the pattern in their data/content (e.g. a
            // ConfigMap whose payload references another resource by name).
            doc.lines().any(|line| {
                let trimmed = line.trim_start();
                trimmed == format!("name: {p}")
                    || trimmed == format!("name: \"{p}\"")
                    || trimmed == format!("name: '{p}'")
            })
        });
        if !should_skip && (has_target_ns || is_target_ns || is_cluster_scoped || is_system_ns) {
            kept.push(doc);
        }
    }
    if kept.is_empty() {
        return String::new();
    }
    format!("---\n{}\n", kept.join("\n---\n"))
}

/// Options controlling a manifest apply operation.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Target namespace; empty string means all namespaces.
    pub namespace: String,
    /// Print rendered YAML without calling kubectl apply.
    pub dry_run: bool,
    /// Patterns whose metadata.name should cause a document to be skipped.
    pub skip_patterns: Vec<String>,
    /// Runtime manifest overrides (--set, --disable, --enable).
    pub overrides: Option<crate::manifest_params::Overrides>,
    /// Domain override. When present, this replaces the active context's
    /// domain for manifest generation. Used when a workflow step (e.g.
    /// EnsureCilium) has discovered a live domain that differs from the
    /// statically-configured one.
    pub domain: Option<String>,
}

/// Discover available service namespaces by scanning `<infra_dir>/base/`
/// for directories that contain a `kustomization.yaml` (or `.yml`).
///
/// Returns a sorted list of directory names. Each name corresponds to a
/// namespace/service that can be passed to `sunbeam service apply <name>`.
pub fn discover_services(infra_dir: &std::path::Path) -> Result<Vec<String>> {
    let base = infra_dir.join("base");
    if !base.exists() {
        bail!(
            "Infrastructure base directory not found: {}",
            base.display()
        );
    }

    let mut services = Vec::new();
    let entries = std::fs::read_dir(&base)
        .with_ctx(|| format!("Failed to read base directory: {}", base.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let has_kustomization =
                path.join("kustomization.yaml").is_file() || path.join("kustomization.yml").is_file();
            if has_kustomization {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    services.push(name.to_string());
                }
            }
        }
    }

    services.sort_unstable();
    Ok(services)
}

/// Build the production kustomize overlay, substitute domain/email, apply via kube-rs.
///
/// Runs a second convergence pass if cert-manager is present in the overlay —
/// cert-manager registers a ValidatingWebhook that must be running before
/// ClusterIssuer / Certificate resources can be created.
///
/// Domain and email are read from `config::active_context()`, which is
/// guaranteed to be fully resolved by the time this is called.
#[tracing::instrument(skip(opts))]
pub async fn apply_manifests(opts: &ApplyOptions) -> Result<String> {
    let ctx = crate::config::active_context();
    let resolved_domain = opts
        .domain
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&ctx.domain);
    let email = &ctx.acme_email;
    if resolved_domain.is_empty() {
        bail!("domain not set — run `sunbeam config set --domain <domain>` first");
    }

    let infra_dir = crate::config::get_infra_dir();
    let overlay = infra_dir.join("overlays");
    let namespace = &opts.namespace;

    let scope = if namespace.is_empty() {
        String::new()
    } else {
        format!(" [{namespace}]")
    };
    tracing::info!("Applying manifests (domain: {resolved_domain}){scope}...");

    // Pre-clean partial helm-chart extracts under any `<base>/charts/` dir.
    clean_partial_chart_extracts(&infra_dir);

    let mut manifests = crate::kube::kustomize_build(&overlay, resolved_domain, email).await?;

    if let Some(ov) = &opts.overrides {
        manifests = crate::manifest_params::apply_overrides(&manifests, ov)?;
    }

    if !namespace.is_empty() {
        manifests = filter_by_namespace(&manifests, namespace, &opts.skip_patterns);
        if manifests.trim().is_empty() {
            tracing::warn!("No resources found for namespace '{namespace}' -- check the name and try again.");
            return Ok(String::new());
        }
    }

    if opts.dry_run {
        return Ok(manifests);
    }

    let ns_list = if namespace.is_empty() {
        None
    } else {
        Some(vec![namespace.clone()])
    };
    pre_apply_cleanup(ns_list.as_deref()).await;

    let before = snapshot_configmaps().await;

    crate::kube::kube_apply(&manifests).await?;

    // If cert-manager is in the overlay, wait for its webhook then re-apply
    let cert_manager_present = overlay.join("../../base/cert-manager").exists();

    if cert_manager_present
        && namespace.is_empty()
        && wait_for_webhook("cert-manager", "cert-manager-webhook", 120).await
    {
        tracing::info!("Running convergence pass for cert-manager resources...");
        let mut manifests2 =
            crate::kube::kustomize_build(&overlay, resolved_domain, email).await?;
        if let Some(ov) = &opts.overrides {
            manifests2 = crate::manifest_params::apply_overrides(&manifests2, ov)?;
        }
        crate::kube::kube_apply(&manifests2).await?;
    }

    restart_for_changed_configmaps(&before, &snapshot_configmaps().await).await;

    // Post-apply hooks
    if namespace.is_empty() || namespace == "matrix" {
        patch_tuwunel_oauth2_redirect(resolved_domain).await;
    }

    tracing::info!("Applied.");
    Ok(manifests)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Walk `<infra_dir>` for any `charts/<chart>-<ver>/` directory whose inner
/// extraction looks incomplete (the inner `<chart>/Chart.yaml` is missing).
/// Remove those partial extracts so the next `kustomize build --enable-helm`
/// re-pulls cleanly.
///
/// Successful extracts are left alone — kustomize happily reuses them.
/// Detection is best-effort: on any read error we just skip that path. We
/// never touch dirs outside `charts/`.
fn clean_partial_chart_extracts(infra_dir: &std::path::Path) {
    fn walk(dir: &std::path::Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some("charts") {
                clean_one_charts_dir(&path);
            } else {
                walk(&path);
            }
        }
    }
    fn clean_one_charts_dir(charts_dir: &std::path::Path) {
        let Ok(entries) = std::fs::read_dir(charts_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let versioned = entry.path();
            if !versioned.is_dir() {
                continue;
            }
            // Each `<chart>-<ver>/` should contain one `<chart>/` dir with
            // a `Chart.yaml` inside. If we find the versioned dir but no
            // valid inner Chart.yaml anywhere, the extract is incomplete.
            let mut has_chart_yaml = false;
            if let Ok(inner) = std::fs::read_dir(&versioned) {
                for inner_entry in inner.flatten() {
                    let inner_path = inner_entry.path();
                    if inner_path.is_dir() && inner_path.join("Chart.yaml").is_file() {
                        has_chart_yaml = true;
                        break;
                    }
                }
            }
            if !has_chart_yaml {
                tracing::warn!(
                    "Removing partial helm-chart extract {}",
                    versioned.display()
                );
                let _ = std::fs::remove_dir_all(&versioned);
            }
        }
    }
    walk(infra_dir);
}

/// Delete immutable resources that must be re-created on each apply.
async fn pre_apply_cleanup(namespaces: Option<&[String]>) {
    let discovered: Vec<String>;
    let ns_list: Vec<&str> = match namespaces {
        Some(ns) => ns.iter().map(|s| s.as_str()).collect(),
        None => {
            discovered = match crate::kube::get_client().await {
                Ok(client) => {
                    let reg = crate::registry::discover(&client).await;
                    reg.map(|r| r.namespaces().into_iter().map(|s| s.to_string()).collect())
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(),
            };
            discovered.iter().map(|s| s.as_str()).collect()
        }
    };

    tracing::info!("Cleaning up immutable Jobs and test Pods...");

    // Prune stale VaultStaticSecrets that share a name with VaultDynamicSecrets
    prune_stale_vault_static_secrets(&ns_list).await;

    for ns in &ns_list {
        // Job drift detection and deletion is handled per-document in
        // kube_apply (spec-hash annotation comparison). Nothing to pre-delete here.

        // Delete test pods
        let client = match crate::kube::get_client().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to get kube client: {e}");
                return;
            }
        };
        let pods: kube::api::Api<k8s_openapi::api::core::v1::Pod> =
            kube::api::Api::namespaced(client.clone(), ns);
        if let Ok(pod_list) = pods.list(&kube::api::ListParams::default()).await {
            for pod in pod_list.items {
                if let Some(name) = &pod.metadata.name
                    && (name.ends_with("-test-connection")
                        || name.ends_with("-server-test")
                        || name.ends_with("-test"))
                {
                    let dp = kube::api::DeleteParams::default();
                    let _ = pods.delete(name, &dp).await;
                }
            }
        }
    }
}

/// Prune VaultStaticSecrets that share a name with VaultDynamicSecrets in the same namespace.
async fn prune_stale_vault_static_secrets(namespaces: &[&str]) {
    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to get kube client for VSS pruning: {e}");
            return;
        }
    };

    let vss_ar = kube::api::ApiResource {
        group: "secrets.hashicorp.com".into(),
        version: "v1beta1".into(),
        api_version: "secrets.hashicorp.com/v1beta1".into(),
        kind: "VaultStaticSecret".into(),
        plural: "vaultstaticsecrets".into(),
    };

    let vds_ar = kube::api::ApiResource {
        group: "secrets.hashicorp.com".into(),
        version: "v1beta1".into(),
        api_version: "secrets.hashicorp.com/v1beta1".into(),
        kind: "VaultDynamicSecret".into(),
        plural: "vaultdynamicsecrets".into(),
    };

    for ns in namespaces {
        let vss_api: kube::api::Api<kube::api::DynamicObject> =
            kube::api::Api::namespaced_with(client.clone(), ns, &vss_ar);
        let vds_api: kube::api::Api<kube::api::DynamicObject> =
            kube::api::Api::namespaced_with(client.clone(), ns, &vds_ar);

        let vss_list = match vss_api.list(&kube::api::ListParams::default()).await {
            Ok(l) => l,
            Err(_) => continue,
        };
        let vds_list = match vds_api.list(&kube::api::ListParams::default()).await {
            Ok(l) => l,
            Err(_) => continue,
        };

        let vds_names: std::collections::HashSet<String> = vds_list
            .items
            .iter()
            .filter_map(|o| o.metadata.name.clone())
            .collect();

        for vss in &vss_list.items {
            if let Some(name) = &vss.metadata.name
                && vds_names.contains(name)
            {
                tracing::info!(
                    "Pruning stale VaultStaticSecret {ns}/{name} (replaced by VaultDynamicSecret)"
                );
                let dp = kube::api::DeleteParams::default();
                let _ = vss_api.delete(name, &dp).await;
            }
        }
    }
}

/// Snapshot ConfigMap resourceVersions across managed namespaces.
async fn snapshot_configmaps() -> std::collections::HashMap<String, String> {
    let mut result = std::collections::HashMap::new();
    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(_) => return result,
    };

    let reg = crate::registry::discover(&client).await;
    let namespaces: Vec<String> = reg
        .map(|r| r.namespaces().into_iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    for ns in &namespaces {
        let cms: kube::api::Api<k8s_openapi::api::core::v1::ConfigMap> =
            kube::api::Api::namespaced(client.clone(), ns);
        if let Ok(cm_list) = cms.list(&kube::api::ListParams::default()).await {
            for cm in cm_list.items {
                if let (Some(name), Some(rv)) = (&cm.metadata.name, &cm.metadata.resource_version) {
                    result.insert(format!("{ns}/{name}"), rv.clone());
                }
            }
        }
    }
    result
}

/// Restart deployments that mount any ConfigMap whose resourceVersion changed.
async fn restart_for_changed_configmaps(
    before: &std::collections::HashMap<String, String>,
    after: &std::collections::HashMap<String, String>,
) {
    let mut changed_by_ns: std::collections::HashMap<&str, std::collections::HashSet<&str>> =
        std::collections::HashMap::new();

    for (key, rv) in after {
        if before.get(key) != Some(rv)
            && let Some((ns, name)) = key.split_once('/')
        {
            changed_by_ns.entry(ns).or_default().insert(name);
        }
    }

    if changed_by_ns.is_empty() {
        return;
    }

    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(_) => return,
    };

    for (ns, cm_names) in &changed_by_ns {
        let deps: kube::api::Api<k8s_openapi::api::apps::v1::Deployment> =
            kube::api::Api::namespaced(client.clone(), ns);
        if let Ok(dep_list) = deps.list(&kube::api::ListParams::default()).await {
            for dep in dep_list.items {
                let dep_name = dep.metadata.name.as_deref().unwrap_or("");
                // Check if this deployment mounts any changed ConfigMap
                let volumes = dep
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.spec.as_ref())
                    .and_then(|s| s.volumes.as_ref());

                if let Some(vols) = volumes {
                    let mounts_changed = vols.iter().any(|v| {
                        if let Some(cm) = &v.config_map {
                            cm_names.contains(cm.name.as_str())
                        } else {
                            false
                        }
                    });
                    if mounts_changed {
                        tracing::info!(
                            "Restarting {ns}/{dep_name} (ConfigMap updated)..."
                        );
                        let _ = crate::kube::kube_rollout_restart(ns, dep_name).await;
                    }
                }
            }
        }
    }
}

/// Wait for a webhook endpoint to become ready.
async fn wait_for_webhook(ns: &str, svc: &str, timeout_secs: u64) -> bool {
    tracing::info!(
        "Waiting for {ns}/{svc} webhook (up to {timeout_secs}s)..."
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(_) => return false,
    };
    let eps: kube::api::Api<k8s_openapi::api::core::v1::Endpoints> =
        kube::api::Api::namespaced(client.clone(), ns);

    loop {
        if std::time::Instant::now() > deadline {
            tracing::warn!(
                "  {ns}/{svc} not ready after {timeout_secs}s -- continuing anyway."
            );
            return false;
        }

        if let Ok(Some(ep)) = eps.get_opt(svc).await {
            let has_addr = ep
                .subsets
                .as_ref()
                .and_then(|ss| ss.first())
                .and_then(|s| s.addresses.as_ref())
                .is_some_and(|a| !a.is_empty());
            if has_addr {
                tracing::info!("  {ns}/{svc} ready.");
                return true;
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// Patch the tuwunel OAuth2Client redirect URI with the actual client_id.
async fn patch_tuwunel_oauth2_redirect(domain: &str) {
    let client_id =
        match crate::kube::kube_get_secret_field("matrix", "oidc-tuwunel", "CLIENT_ID").await {
            Ok(id) if !id.is_empty() => id,
            _ => {
                tracing::warn!(
                    "oidc-tuwunel secret not yet available -- skipping redirect URI patch.",
                );
                return;
            }
        };

    let redirect_uri =
        format!("https://messages.{domain}/_matrix/client/unstable/login/sso/callback/{client_id}");

    // Patch the OAuth2Client CRD via kube-rs
    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(_) => return,
    };

    let ar = kube::api::ApiResource {
        group: "hydra.ory.sh".into(),
        version: "v1alpha1".into(),
        api_version: "hydra.ory.sh/v1alpha1".into(),
        kind: "OAuth2Client".into(),
        plural: "oauth2clients".into(),
    };

    let api: kube::api::Api<kube::api::DynamicObject> =
        kube::api::Api::namespaced_with(client.clone(), "matrix", &ar);

    let patch = serde_json::json!({
        "spec": {
            "redirectUris": [redirect_uri]
        }
    });

    let pp = kube::api::PatchParams::default();
    if let Err(e) = api
        .patch("tuwunel", &pp, &kube::api::Patch::Merge(patch))
        .await
    {
        tracing::warn!("Failed to patch tuwunel OAuth2Client: {e}");
    } else {
        tracing::info!("Patched tuwunel OAuth2Client redirect URI.");
    }
}

// ---------------------------------------------------------------------------
// OpenSearch helpers (kube exec + curl inside pod)
// ---------------------------------------------------------------------------

/// Call OpenSearch API via kube exec curl inside the opensearch pod.
#[allow(dead_code)]
async fn os_api(path: &str, method: &str, body: Option<&str>) -> Option<String> {
    let url = format!("http://localhost:9200{path}");
    let mut curl_args: Vec<&str> = vec!["curl", "-sf", &url];
    if method != "GET" {
        curl_args.extend_from_slice(&["-X", method]);
    }
    let body_string;
    if let Some(b) = body {
        body_string = b.to_string();
        curl_args.extend_from_slice(&["-H", "Content-Type: application/json", "-d", &body_string]);
    }

    // Resolve the actual pod name from the app=opensearch label
    let pod_name = match crate::kube::find_pod_by_label("data", "app=opensearch").await {
        Some(name) => name,
        None => {
            tracing::warn!("No OpenSearch pod found in data namespace");
            return None;
        }
    };

    match crate::kube::kube_exec("data", &pod_name, &curl_args, Some("opensearch")).await {
        Ok((0, out)) if !out.is_empty() => Some(out),
        _ => None,
    }
}

/// Inject OpenSearch model_id into matrix/opensearch-ml-config ConfigMap.
#[tracing::instrument]
pub async fn inject_opensearch_model_id() {
    let pipe_resp = match os_api("/_ingest/pipeline/tuwunel_embedding_pipeline", "GET", None).await
    {
        Some(r) => r,
        None => {
            tracing::warn!(
                "OpenSearch ingest pipeline not found -- skipping model_id injection.",
            );
            return;
        }
    };

    let model_id = serde_json::from_str::<serde_json::Value>(&pipe_resp)
        .ok()
        .and_then(|v| {
            v.get("tuwunel_embedding_pipeline")?
                .get("processors")?
                .as_array()?
                .iter()
                .find_map(|p| {
                    p.get("text_embedding")?
                        .get("model_id")?
                        .as_str()
                        .map(String::from)
                })
        });

    let Some(model_id) = model_id else {
        tracing::warn!("No model_id in ingest pipeline -- tuwunel hybrid search unavailable.");
        return;
    };

    // Check if ConfigMap already has this value
    if let Ok(current) =
        crate::kube::kube_get_secret_field("matrix", "opensearch-ml-config", "model_id").await
        && current == model_id
    {
        return;
    }

    let cm = serde_json::json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {"name": "opensearch-ml-config", "namespace": "matrix"},
        "data": {"model_id": &model_id},
    });

    let manifest = serde_json::to_string(&cm).unwrap_or_default();
    if let Err(e) = crate::kube::kube_apply(&manifest).await {
        tracing::warn!("Failed to inject OpenSearch model_id: {e}");
    } else {
        tracing::info!(
            "Injected OpenSearch model_id ({model_id}) into matrix/opensearch-ml-config."
        );
    }
}

/// Configure OpenSearch ML Commons for neural search.
///
/// 1. Sets cluster settings to allow ML on data nodes.
/// 2. Registers and deploys all-mpnet-base-v2 (pre-trained, 384-dim).
/// 3. Creates ingest + search pipelines for hybrid BM25+neural scoring.
#[tracing::instrument]
pub async fn ensure_opensearch_ml() {
    if os_api("/_cluster/health", "GET", None).await.is_none() {
        tracing::warn!("OpenSearch not reachable -- skipping ML setup.");
        return;
    }

    // 1. ML Commons cluster settings
    let settings = serde_json::json!({
        "persistent": {
            "plugins.ml_commons.only_run_on_ml_node": false,
            "plugins.ml_commons.native_memory_threshold": 90,
            "plugins.ml_commons.model_access_control_enabled": false,
            "plugins.ml_commons.allow_registering_model_via_url": true,
        }
    });
    os_api(
        "/_cluster/settings",
        "PUT",
        Some(&serde_json::to_string(&settings).unwrap()),
    )
    .await;

    // 2. Check if model already registered and deployed
    let search_body =
        r#"{"query":{"match":{"name":"huggingface/sentence-transformers/all-mpnet-base-v2"}}}"#;
    let search_resp = match os_api("/_plugins/_ml/models/_search", "POST", Some(search_body)).await
    {
        Some(r) => r,
        None => {
            tracing::warn!("OpenSearch ML search API failed -- skipping ML setup.");
            return;
        }
    };

    let resp: serde_json::Value = match serde_json::from_str(&search_resp) {
        Ok(v) => v,
        Err(_) => return,
    };

    let hits = resp
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default();

    // Categorise all matching models by state.
    let mut deployed_ids: Vec<String> = Vec::new();
    let mut deploying_ids: Vec<String> = Vec::new();
    let mut registered_ids: Vec<String> = Vec::new();
    let mut stale_ids: Vec<String> = Vec::new(); // FAILED, DEPLOY_FAILED, etc.

    for hit in &hits {
        let state = hit
            .get("_source")
            .and_then(|s| s.get("model_state"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let id = hit.get("_id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        match state {
            "DEPLOYED" => deployed_ids.push(id.to_string()),
            "DEPLOYING" => deploying_ids.push(id.to_string()),
            "REGISTERED" => registered_ids.push(id.to_string()),
            _ => stale_ids.push(id.to_string()),
        }
    }

    // Pick the best model: first DEPLOYED, then first DEPLOYING, then first REGISTERED.
    let best_id = deployed_ids
        .first()
        .or(deploying_ids.first())
        .or(registered_ids.first())
        .cloned();

    // Clean up duplicates: everything that isn't the best model.
    let mut to_clean: Vec<String> = stale_ids; // always clean FAILED/stale
    if let Some(ref best) = best_id {
        for id in &deployed_ids {
            if id != best {
                to_clean.push(id.clone());
            }
        }
        for id in &deploying_ids {
            if id != best {
                to_clean.push(id.clone());
            }
        }
        for id in &registered_ids {
            if id != best {
                to_clean.push(id.clone());
            }
        }
    }

    if !to_clean.is_empty() {
        tracing::info!(
            "Cleaning up {} stale ML model(s)...",
            to_clean.len()
        );
        for stale in &to_clean {
            // Undeploy first (safe to call even if not deployed)
            os_api(
                &format!("/_plugins/_ml/models/{stale}/_undeploy"),
                "POST",
                None,
            )
            .await;
            // Then delete
            os_api(&format!("/_plugins/_ml/models/{stale}"), "DELETE", None).await;
        }
    }

    let mut model_id: Option<String> = None;

    if let Some(id) = best_id {
        // Check current state of the chosen model.
        let state = hits
            .iter()
            .find(|h| h.get("_id").and_then(|v| v.as_str()) == Some(&id))
            .and_then(|h| h.get("_source"))
            .and_then(|s| s.get("model_state"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match state {
            "DEPLOYED" => {
                // Nothing to do.
                model_id = Some(id);
            }
            "DEPLOYING" => {
                tracing::info!("Model is deploying, waiting...");
                model_id = Some(id.clone());
                for _ in 0..30 {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if let Some(r) =
                        os_api(&format!("/_plugins/_ml/models/{id}"), "GET", None).await
                        && r.contains("\"DEPLOYED\"")
                    {
                        break;
                    }
                }
            }
            _ => {
                // REGISTERED or other — deploy it.
                tracing::info!("Deploying OpenSearch ML model...");
                model_id = Some(id.clone());
                os_api(&format!("/_plugins/_ml/models/{id}/_deploy"), "POST", None).await;
                for _ in 0..30 {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if let Some(r) =
                        os_api(&format!("/_plugins/_ml/models/{id}"), "GET", None).await
                        && r.contains("\"DEPLOYED\"")
                    {
                        break;
                    }
                }
            }
        }
    }

    if model_id.is_none() {
        // No existing model found — register from pre-trained hub
        tracing::info!("Registering OpenSearch ML model (all-mpnet-base-v2)...");
        let reg_body = serde_json::json!({
            "name": "huggingface/sentence-transformers/all-mpnet-base-v2",
            "version": "1.0.1",
            "model_format": "TORCH_SCRIPT",
        });
        let reg_resp = match os_api(
            "/_plugins/_ml/models/_register",
            "POST",
            Some(&serde_json::to_string(&reg_body).unwrap()),
        )
        .await
        {
            Some(r) => r,
            None => {
                tracing::warn!("Failed to register ML model -- skipping.");
                return;
            }
        };

        let task_id = serde_json::from_str::<serde_json::Value>(&reg_resp)
            .ok()
            .and_then(|v| v.get("task_id")?.as_str().map(String::from))
            .unwrap_or_default();

        if task_id.is_empty() {
            tracing::warn!("No task_id from model registration -- skipping.");
            return;
        }

        tracing::info!("Waiting for model registration...");
        let mut new_model_id = None;
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            if let Some(task_resp) =
                os_api(&format!("/_plugins/_ml/tasks/{task_id}"), "GET", None).await
                && let Ok(task) = serde_json::from_str::<serde_json::Value>(&task_resp)
            {
                match task.get("state").and_then(|v| v.as_str()).unwrap_or("") {
                    "COMPLETED" => {
                        new_model_id = task
                            .get("model_id")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        break;
                    }
                    "FAILED" => {
                        tracing::warn!("ML model registration failed: {task_resp}");
                        return;
                    }
                    _ => {}
                }
            }
        }

        let Some(mid) = new_model_id else {
            tracing::warn!("ML model registration timed out.");
            return;
        };

        tracing::info!("Deploying ML model...");
        os_api(&format!("/_plugins/_ml/models/{mid}/_deploy"), "POST", None).await;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            if let Some(r) = os_api(&format!("/_plugins/_ml/models/{mid}"), "GET", None).await
                && r.contains("\"DEPLOYED\"")
            {
                break;
            }
        }
        model_id = Some(mid);
    }

    let Some(model_id) = model_id else {
        tracing::warn!("No ML model available -- skipping pipeline setup.");
        return;
    };

    // 3. Ingest pipeline
    let ingest = serde_json::json!({
        "description": "Tuwunel message embedding pipeline",
        "processors": [{"text_embedding": {
            "model_id": &model_id,
            "field_map": {"body": "embedding"},
        }}],
    });
    os_api(
        "/_ingest/pipeline/tuwunel_embedding_pipeline",
        "PUT",
        Some(&serde_json::to_string(&ingest).unwrap()),
    )
    .await;

    // 4. Search pipeline
    let search = serde_json::json!({
        "description": "Tuwunel hybrid BM25+neural search pipeline",
        "phase_results_processors": [{"normalization-processor": {
            "normalization": {"technique": "min_max"},
            "combination": {
                "technique": "arithmetic_mean",
                "parameters": {"weights": [0.3, 0.7]},
            },
        }}],
    });
    os_api(
        "/_search/pipeline/tuwunel_hybrid_pipeline",
        "PUT",
        Some(&serde_json::to_string(&search).unwrap()),
    )
    .await;

    tracing::info!("OpenSearch ML ready (model: {model_id}).");
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_DOC: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: stalwart-config
  namespace: stalwart
data:
  FOO: bar
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: stalwart
  namespace: stalwart
spec:
  replicas: 1
---
apiVersion: v1
kind: Namespace
metadata:
  name: stalwart
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: pingora-config
  namespace: ingress
data:
  config.toml: |
    hello
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: pingora
  namespace: ingress
spec:
  replicas: 1
";

    #[test]
    fn test_keeps_matching_namespace() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart", &[]);
        assert!(result.contains("name: stalwart-config"));
        assert!(result.contains("name: stalwart\n"));
    }

    #[test]
    fn test_excludes_other_namespaces() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart", &[]);
        assert!(!result.contains("namespace: ingress"));
        assert!(!result.contains("name: pingora-config"));
        assert!(!result.contains("name: pingora\n"));
    }

    #[test]
    fn test_includes_namespace_resource_itself() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart", &[]);
        assert!(result.contains("kind: Namespace"));
    }

    #[test]
    fn test_ingress_filter() {
        let result = filter_by_namespace(MULTI_DOC, "ingress", &[]);
        assert!(result.contains("name: pingora-config"));
        assert!(result.contains("name: pingora"));
        assert!(!result.contains("namespace: stalwart"));
    }

    #[test]
    fn test_unknown_namespace_returns_empty() {
        let result = filter_by_namespace(MULTI_DOC, "nonexistent", &[]);
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_empty_input_returns_empty() {
        let result = filter_by_namespace("", "stalwart", &[]);
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_result_starts_with_separator() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart", &[]);
        assert!(result.starts_with("---"));
    }

    #[test]
    fn test_does_not_include_namespace_resource_for_wrong_ns() {
        let result = filter_by_namespace(MULTI_DOC, "ingress", &[]);
        assert!(!result.contains("kind: Namespace"));
    }

    #[test]
    fn test_single_doc_matching() {
        let doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n";
        let result = filter_by_namespace(doc, "ory", &[]);
        assert!(result.contains("name: x"));
    }

    #[test]
    fn test_single_doc_not_matching() {
        let doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n";
        let result = filter_by_namespace(doc, "stalwart", &[]);
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_skip_by_name() {
        let doc = "---\napiVersion: v1\nkind: Secret\nmetadata:\n  name: pingora-tls\n  namespace: ingress\n";
        let result = filter_by_namespace(doc, "ingress", &["pingora-tls".to_string()]);
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_skip_does_not_match_data_content() {
        // A ConfigMap that mentions pingora-tls in its data should NOT be
        // skipped — the skip pattern must match metadata.name.
        let doc = "---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: pingora-config\n  namespace: ingress\ndata:\n  config.toml: |\n    tls_secret = \"pingora-tls\"\n";
        let result = filter_by_namespace(doc, "ingress", &["pingora-tls".to_string()]);
        assert!(result.contains("name: pingora-config"));
    }

    // -----------------------------------------------------------------------
    // ApplyOptions tests
    // -----------------------------------------------------------------------

    #[test]
    fn apply_options_default_is_all_namespaces_no_dry_run() {
        let opts = ApplyOptions::default();
        assert!(opts.namespace.is_empty());
        assert!(!opts.dry_run);
        assert!(opts.skip_patterns.is_empty());
        assert!(opts.overrides.is_none());
        assert!(opts.domain.is_none());
    }

    #[test]
    fn apply_options_with_namespace_and_overrides() {
        let overrides = crate::manifest_params::Overrides::from_cli(
            &["Deployment/ory/hydra/spec/replicas=3".to_string()],
            &["deployment/ory/hydra".to_string()],
            &[],
        )
        .unwrap();
        let opts = ApplyOptions {
            namespace: "ory".to_string(),
            dry_run: true,
            skip_patterns: vec!["scaleway-certmanager-webhook".to_string()],
            overrides: Some(overrides),
            domain: None,
        };
        assert_eq!(opts.namespace, "ory");
        assert!(opts.dry_run);
        assert_eq!(opts.skip_patterns.len(), 1);
        assert!(opts.overrides.is_some());
    }

    // -----------------------------------------------------------------------
    // discover_services tests
    // -----------------------------------------------------------------------

    use std::fs;

    fn make_dir_with_kustomization(base: &std::path::Path, name: &str, ext: &str) {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("kustomization.{ext}")), "resources: []\n").unwrap();
    }

    #[test]
    fn discover_services_empty_base_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        let found = discover_services(tmp.path()).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn discover_services_missing_base_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = discover_services(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("base directory not found"));
    }

    #[test]
    fn discover_services_finds_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "ory", "yaml");
        make_dir_with_kustomization(&base, "data", "yaml");
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["data", "ory"]);
    }

    #[test]
    fn discover_services_finds_yml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "matrix", "yml");
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["matrix"]);
    }

    #[test]
    fn discover_services_ignores_dirs_without_kustomization() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "ory", "yaml");
        fs::create_dir(base.join("not-a-service")).unwrap();
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["ory"]);
    }

    #[test]
    fn discover_services_ignores_files_in_base() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "monitoring", "yaml");
        fs::write(base.join("README.md"), "# base\n").unwrap();
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["monitoring"]);
    }

    #[test]
    fn discover_services_sorts_alphabetically() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "wfe", "yaml");
        make_dir_with_kustomization(&base, "ory", "yaml");
        make_dir_with_kustomization(&base, "data", "yaml");
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["data", "ory", "wfe"]);
    }

    #[test]
    fn discover_services_ignores_nested_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "stalwart", "yaml");
        // Nested directory inside a service dir — should not be listed
        fs::create_dir_all(base.join("stalwart").join("sub-component")).unwrap();
        fs::write(
            base.join("stalwart").join("sub-component").join("kustomization.yaml"),
            "resources: []\n",
        )
        .unwrap();
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["stalwart"]);
    }

    #[test]
    fn discover_services_yaml_takes_precedence_over_yml() {
        // A directory with BOTH files should still only be listed once.
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        let dir = base.join("press");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("kustomization.yaml"), "resources: []\n").unwrap();
        fs::write(dir.join("kustomization.yml"), "resources: []\n").unwrap();
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["press"]);
    }

    #[test]
    fn discover_services_empty_dir_name_skipped() {
        // Directories are created normally; impossible to have an empty OS dir name.
        // This test verifies the file_name parsing branch is safe.
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        make_dir_with_kustomization(&base, "vpn", "yaml");
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["vpn"]);
    }

    #[test]
    fn discover_services_symlink_to_dir_followed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        let real_dir = tmp.path().join("real-ingress");
        fs::create_dir(&real_dir).unwrap();
        fs::write(real_dir.join("kustomization.yaml"), "resources: []\n").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_dir, base.join("ingress")).unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(&real_dir, base.join("ingress")).unwrap();
        }
        let found = discover_services(tmp.path()).unwrap();
        assert_eq!(found, vec!["ingress"]);
    }

    #[test]
    fn discover_services_real_infra_sbbb_has_expected_services() {
        // Sanity-check against the actual on-disk infrastructure directory.
        let infra = std::path::PathBuf::from("../../../infra/sbbb");
        if !infra.exists() {
            // Skip when running outside the monorepo (e.g. published crate tests).
            return;
        }
        let found = discover_services(&infra).unwrap();
        assert!(
            found.contains(&"ory".to_string()),
            "expected 'ory' in discovered services"
        );
        assert!(
            found.contains(&"data".to_string()),
            "expected 'data' in discovered services"
        );
        assert!(
            found.contains(&"matrix".to_string()),
            "expected 'matrix' in discovered services"
        );
        // Should NOT contain arbitrary non-service directories
        assert!(!found.contains(&"not-a-service".to_string()));
    }
}
