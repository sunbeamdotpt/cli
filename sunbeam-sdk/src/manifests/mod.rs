use crate::error::Result;
use crate::constants::MANAGED_NS;

/// Return only the YAML documents that belong to the given namespace.
pub fn filter_by_namespace(manifests: &str, namespace: &str) -> String {
    let mut kept = Vec::new();
    for doc in manifests.split("\n---") {
        let doc = doc.trim();
        if doc.is_empty() {
            continue;
        }
        let has_ns = doc.contains(&format!("namespace: {namespace}"));
        let is_ns_resource =
            doc.contains("kind: Namespace") && doc.contains(&format!("name: {namespace}"));
        if has_ns || is_ns_resource {
            kept.push(doc);
        }
    }
    if kept.is_empty() {
        return String::new();
    }
    format!("---\n{}\n", kept.join("\n---\n"))
}

/// Build kustomize overlay for env, substitute domain/email, apply via kube-rs.
///
/// Runs a second convergence pass if cert-manager is present in the overlay —
/// cert-manager registers a ValidatingWebhook that must be running before
/// ClusterIssuer / Certificate resources can be created.
pub async fn cmd_apply(env: &str, domain: &str, email: &str, namespace: &str) -> Result<()> {
    // Fall back to config for ACME email if not provided via CLI flag.
    let email = if email.is_empty() {
        crate::config::load_config().acme_email
    } else {
        email.to_string()
    };

    let infra_dir = crate::config::get_infra_dir();

    let (resolved_domain, overlay) = if env == "production" {
        let d = if domain.is_empty() {
            crate::kube::get_domain().await?
        } else {
            domain.to_string()
        };
        if d.is_empty() {
            bail!("--domain is required for production apply on first deploy");
        }
        let overlay = infra_dir.join("overlays").join("production");
        (d, overlay)
    } else {
        // Local: discover domain from Lima IP
        let d = crate::kube::get_domain().await?;
        let overlay = infra_dir.join("overlays").join("local");
        (d, overlay)
    };

    let scope = if namespace.is_empty() {
        String::new()
    } else {
        format!(" [{namespace}]")
    };
    crate::output::step(&format!(
        "Applying manifests (env: {env}, domain: {resolved_domain}){scope}..."
    ));

    if env == "local" {
        apply_mkcert_ca_configmap().await;
    }

    let ns_list = if namespace.is_empty() {
        None
    } else {
        Some(vec![namespace.to_string()])
    };
    pre_apply_cleanup(ns_list.as_deref()).await;

    let before = snapshot_configmaps().await;
    let mut manifests =
        crate::kube::kustomize_build(&overlay, &resolved_domain, &email).await?;

    if !namespace.is_empty() {
        manifests = filter_by_namespace(&manifests, namespace);
        if manifests.trim().is_empty() {
            crate::output::warn(&format!(
                "No resources found for namespace '{namespace}' -- check the name and try again."
            ));
            return Ok(());
        }
    }

    // First pass: may emit errors for resources that depend on webhooks not yet running
    if let Err(e) = crate::kube::kube_apply(&manifests).await {
        crate::output::warn(&format!("First apply pass had errors (may be expected): {e}"));
    }

    // If cert-manager is in the overlay, wait for its webhook then re-apply
    let cert_manager_present = overlay
        .join("../../base/cert-manager")
        .exists();

    if cert_manager_present && namespace.is_empty() {
        if wait_for_webhook("cert-manager", "cert-manager-webhook", 120).await {
            crate::output::ok("Running convergence pass for cert-manager resources...");
            let manifests2 =
                crate::kube::kustomize_build(&overlay, &resolved_domain, &email).await?;
            crate::kube::kube_apply(&manifests2).await?;
        }
    }

    restart_for_changed_configmaps(&before, &snapshot_configmaps().await).await;

    // Post-apply hooks
    if namespace.is_empty() || namespace == "matrix" {
        patch_tuwunel_oauth2_redirect(&resolved_domain).await;
        inject_opensearch_model_id().await;
    }
    if namespace.is_empty() || namespace == "data" {
        ensure_opensearch_ml().await;
    }

    crate::output::ok("Applied.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Delete immutable resources that must be re-created on each apply.
async fn pre_apply_cleanup(namespaces: Option<&[String]>) {
    let ns_list: Vec<&str> = match namespaces {
        Some(ns) => ns.iter().map(|s| s.as_str()).collect(),
        None => MANAGED_NS.to_vec(),
    };

    crate::output::ok("Cleaning up immutable Jobs and test Pods...");

    // Prune stale VaultStaticSecrets that share a name with VaultDynamicSecrets
    prune_stale_vault_static_secrets(&ns_list).await;

    for ns in &ns_list {
        // Delete all jobs
        let client = match crate::kube::get_client().await {
            Ok(c) => c,
            Err(e) => {
                crate::output::warn(&format!("Failed to get kube client: {e}"));
                return;
            }
        };
        let jobs: kube::api::Api<k8s_openapi::api::batch::v1::Job> =
            kube::api::Api::namespaced(client.clone(), ns);
        if let Ok(job_list) = jobs.list(&kube::api::ListParams::default()).await {
            for job in job_list.items {
                if let Some(name) = &job.metadata.name {
                    let dp = kube::api::DeleteParams::default();
                    let _ = jobs.delete(name, &dp).await;
                }
            }
        }

        // Delete test pods
        let pods: kube::api::Api<k8s_openapi::api::core::v1::Pod> =
            kube::api::Api::namespaced(client.clone(), ns);
        if let Ok(pod_list) = pods.list(&kube::api::ListParams::default()).await {
            for pod in pod_list.items {
                if let Some(name) = &pod.metadata.name {
                    if name.ends_with("-test-connection")
                        || name.ends_with("-server-test")
                        || name.ends_with("-test")
                    {
                        let dp = kube::api::DeleteParams::default();
                        let _ = pods.delete(name, &dp).await;
                    }
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
            crate::output::warn(&format!("Failed to get kube client for VSS pruning: {e}"));
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
            if let Some(name) = &vss.metadata.name {
                if vds_names.contains(name) {
                    crate::output::ok(&format!(
                        "Pruning stale VaultStaticSecret {ns}/{name} (replaced by VaultDynamicSecret)"
                    ));
                    let dp = kube::api::DeleteParams::default();
                    let _ = vss_api.delete(name, &dp).await;
                }
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

    for ns in MANAGED_NS {
        let cms: kube::api::Api<k8s_openapi::api::core::v1::ConfigMap> =
            kube::api::Api::namespaced(client.clone(), ns);
        if let Ok(cm_list) = cms.list(&kube::api::ListParams::default()).await {
            for cm in cm_list.items {
                if let (Some(name), Some(rv)) = (
                    &cm.metadata.name,
                    &cm.metadata.resource_version,
                ) {
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
        if before.get(key) != Some(rv) {
            if let Some((ns, name)) = key.split_once('/') {
                changed_by_ns.entry(ns).or_default().insert(name);
            }
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
                        crate::output::ok(&format!(
                            "Restarting {ns}/{dep_name} (ConfigMap updated)..."
                        ));
                        let _ = crate::kube::kube_rollout_restart(ns, dep_name).await;
                    }
                }
            }
        }
    }
}

/// Wait for a webhook endpoint to become ready.
async fn wait_for_webhook(ns: &str, svc: &str, timeout_secs: u64) -> bool {
    crate::output::ok(&format!(
        "Waiting for {ns}/{svc} webhook (up to {timeout_secs}s)..."
    ));
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(_) => return false,
    };
    let eps: kube::api::Api<k8s_openapi::api::core::v1::Endpoints> =
        kube::api::Api::namespaced(client.clone(), ns);

    loop {
        if std::time::Instant::now() > deadline {
            crate::output::warn(&format!(
                "  {ns}/{svc} not ready after {timeout_secs}s -- continuing anyway."
            ));
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
                crate::output::ok(&format!("  {ns}/{svc} ready."));
                return true;
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// Create/update gitea-mkcert-ca ConfigMap from the local mkcert root CA.
async fn apply_mkcert_ca_configmap() {
    let caroot = tokio::process::Command::new("mkcert")
        .arg("-CAROOT")
        .output()
        .await;

    let caroot_path = match caroot {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => {
            crate::output::warn("mkcert not found -- skipping gitea-mkcert-ca ConfigMap.");
            return;
        }
    };

    let ca_pem_path = std::path::Path::new(&caroot_path).join("rootCA.pem");
    let ca_pem = match std::fs::read_to_string(&ca_pem_path) {
        Ok(s) => s,
        Err(_) => {
            crate::output::warn(&format!(
                "mkcert root CA not found at {} -- skipping.",
                ca_pem_path.display()
            ));
            return;
        }
    };

    let cm = serde_json::json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {"name": "gitea-mkcert-ca", "namespace": "devtools"},
        "data": {"ca.crt": ca_pem},
    });

    let manifest = serde_json::to_string(&cm).unwrap_or_default();
    if let Err(e) = crate::kube::kube_apply(&manifest).await {
        crate::output::warn(&format!("Failed to apply gitea-mkcert-ca: {e}"));
    } else {
        crate::output::ok("gitea-mkcert-ca ConfigMap applied.");
    }
}

/// Patch the tuwunel OAuth2Client redirect URI with the actual client_id.
async fn patch_tuwunel_oauth2_redirect(domain: &str) {
    let client_id = match crate::kube::kube_get_secret_field("matrix", "oidc-tuwunel", "CLIENT_ID")
        .await
    {
        Ok(id) if !id.is_empty() => id,
        _ => {
            crate::output::warn(
                "oidc-tuwunel secret not yet available -- skipping redirect URI patch.",
            );
            return;
        }
    };

    let redirect_uri = format!(
        "https://messages.{domain}/_matrix/client/unstable/login/sso/callback/{client_id}"
    );

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
        crate::output::warn(&format!("Failed to patch tuwunel OAuth2Client: {e}"));
    } else {
        crate::output::ok("Patched tuwunel OAuth2Client redirect URI.");
    }
}

// ---------------------------------------------------------------------------
// OpenSearch helpers (kube exec + curl inside pod)
// ---------------------------------------------------------------------------

/// Call OpenSearch API via kube exec curl inside the opensearch pod.
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

    let pod_name = match crate::kube::find_pod_by_label("data", "app=opensearch").await {
        Some(name) => name,
        None => {
            crate::output::warn("No OpenSearch pod found in data namespace");
            return None;
        }
    };

    match crate::kube::kube_exec("data", &pod_name, &curl_args, Some("opensearch")).await {
        Ok((0, out)) if !out.is_empty() => Some(out),
        _ => None,
    }
}

/// Inject OpenSearch model_id into matrix/opensearch-ml-config ConfigMap.
async fn inject_opensearch_model_id() {
    let pipe_resp =
        match os_api("/_ingest/pipeline/tuwunel_embedding_pipeline", "GET", None).await {
            Some(r) => r,
            None => {
                crate::output::warn(
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
        crate::output::warn(
            "No model_id in ingest pipeline -- tuwunel hybrid search unavailable.",
        );
        return;
    };

    // Check if ConfigMap already has this value
    if let Ok(current) =
        crate::kube::kube_get_secret_field("matrix", "opensearch-ml-config", "model_id").await
    {
        if current == model_id {
            return;
        }
    }

    let cm = serde_json::json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {"name": "opensearch-ml-config", "namespace": "matrix"},
        "data": {"model_id": &model_id},
    });

    let manifest = serde_json::to_string(&cm).unwrap_or_default();
    if let Err(e) = crate::kube::kube_apply(&manifest).await {
        crate::output::warn(&format!("Failed to inject OpenSearch model_id: {e}"));
    } else {
        crate::output::ok(&format!(
            "Injected OpenSearch model_id ({model_id}) into matrix/opensearch-ml-config."
        ));
    }
}

/// Configure OpenSearch ML Commons for neural search.
///
/// 1. Sets cluster settings to allow ML on data nodes.
/// 2. Registers and deploys all-mpnet-base-v2 (pre-trained, 384-dim).
/// 3. Creates ingest + search pipelines for hybrid BM25+neural scoring.
async fn ensure_opensearch_ml() {
    if os_api("/_cluster/health", "GET", None).await.is_none() {
        crate::output::warn("OpenSearch not reachable -- skipping ML setup.");
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
            crate::output::warn("OpenSearch ML search API failed -- skipping ML setup.");
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

    let mut model_id: Option<String> = None;
    let mut already_deployed = false;

    for hit in &hits {
        let state = hit
            .get("_source")
            .and_then(|s| s.get("model_state"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let id = hit.get("_id").and_then(|v| v.as_str()).unwrap_or("");
        match state {
            "DEPLOYED" => {
                model_id = Some(id.to_string());
                already_deployed = true;
                break;
            }
            // Any existing model (even DEPLOY_FAILED) — reuse it instead of
            // registering a new version. This prevents accumulating stale
            // copies in .plugins-ml-model when the pod restarts.
            _ => {
                if model_id.is_none() && !id.is_empty() {
                    model_id = Some(id.to_string());
                }
            }
        }
    }

    if !already_deployed {
        if let Some(ref mid) = model_id {
            // Registered but not deployed -- deploy it
            crate::output::ok("Deploying OpenSearch ML model...");
            os_api(
                &format!("/_plugins/_ml/models/{mid}/_deploy"),
                "POST",
                None,
            )
            .await;
            for _ in 0..30 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Some(r) =
                    os_api(&format!("/_plugins/_ml/models/{mid}"), "GET", None).await
                {
                    if r.contains("\"DEPLOYED\"") {
                        break;
                    }
                }
            }
        } else {
            // Register from pre-trained hub
            crate::output::ok("Registering OpenSearch ML model (all-mpnet-base-v2)...");
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
                    crate::output::warn("Failed to register ML model -- skipping.");
                    return;
                }
            };

            let task_id = serde_json::from_str::<serde_json::Value>(&reg_resp)
                .ok()
                .and_then(|v| v.get("task_id")?.as_str().map(String::from))
                .unwrap_or_default();

            if task_id.is_empty() {
                crate::output::warn("No task_id from model registration -- skipping.");
                return;
            }

            crate::output::ok("Waiting for model registration...");
            let mut registered_id = None;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                if let Some(task_resp) =
                    os_api(&format!("/_plugins/_ml/tasks/{task_id}"), "GET", None).await
                {
                    if let Ok(task) = serde_json::from_str::<serde_json::Value>(&task_resp) {
                        match task.get("state").and_then(|v| v.as_str()).unwrap_or("") {
                            "COMPLETED" => {
                                registered_id = task
                                    .get("model_id")
                                    .and_then(|v| v.as_str())
                                    .map(String::from);
                                break;
                            }
                            "FAILED" => {
                                crate::output::warn(&format!(
                                    "ML model registration failed: {task_resp}"
                                ));
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            let Some(mid) = registered_id else {
                crate::output::warn("ML model registration timed out.");
                return;
            };

            crate::output::ok("Deploying ML model...");
            os_api(
                &format!("/_plugins/_ml/models/{mid}/_deploy"),
                "POST",
                None,
            )
            .await;
            for _ in 0..30 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Some(r) =
                    os_api(&format!("/_plugins/_ml/models/{mid}"), "GET", None).await
                {
                    if r.contains("\"DEPLOYED\"") {
                        break;
                    }
                }
            }
            model_id = Some(mid);
        }
    }

    let Some(model_id) = model_id else {
        crate::output::warn("No ML model available -- skipping pipeline setup.");
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

    crate::output::ok(&format!("OpenSearch ML ready (model: {model_id})."));
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
        let result = filter_by_namespace(MULTI_DOC, "stalwart");
        assert!(result.contains("name: stalwart-config"));
        assert!(result.contains("name: stalwart\n"));
    }

    #[test]
    fn test_excludes_other_namespaces() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart");
        assert!(!result.contains("namespace: ingress"));
        assert!(!result.contains("name: pingora-config"));
        assert!(!result.contains("name: pingora\n"));
    }

    #[test]
    fn test_includes_namespace_resource_itself() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart");
        assert!(result.contains("kind: Namespace"));
    }

    #[test]
    fn test_ingress_filter() {
        let result = filter_by_namespace(MULTI_DOC, "ingress");
        assert!(result.contains("name: pingora-config"));
        assert!(result.contains("name: pingora"));
        assert!(!result.contains("namespace: stalwart"));
    }

    #[test]
    fn test_unknown_namespace_returns_empty() {
        let result = filter_by_namespace(MULTI_DOC, "nonexistent");
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_empty_input_returns_empty() {
        let result = filter_by_namespace("", "stalwart");
        assert!(result.trim().is_empty());
    }

    #[test]
    fn test_result_starts_with_separator() {
        let result = filter_by_namespace(MULTI_DOC, "stalwart");
        assert!(result.starts_with("---"));
    }

    #[test]
    fn test_does_not_include_namespace_resource_for_wrong_ns() {
        let result = filter_by_namespace(MULTI_DOC, "ingress");
        assert!(!result.contains("kind: Namespace"));
    }

    #[test]
    fn test_single_doc_matching() {
        let doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n";
        let result = filter_by_namespace(doc, "ory");
        assert!(result.contains("name: x"));
    }

    #[test]
    fn test_single_doc_not_matching() {
        let doc = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n  namespace: ory\n";
        let result = filter_by_namespace(doc, "stalwart");
        assert!(result.trim().is_empty());
    }
}
