//! Cluster tear-down — deletes all Sunbeam-managed namespaces.

use crate::error::{Result, SunbeamError};
use kube::api::{Api, Patch, PatchParams};

/// Namespaces managed by Sunbeam infrastructure.
pub const INFRA_NAMESPACES: &[&str] = &["cert-manager", "longhorn-system"];

/// Namespaces managed by Sunbeam applications.
pub const APP_NAMESPACES: &[&str] = &[
    "build",
    "data",
    "devtools",
    "ingress",
    "matrix",
    "media",
    "monitoring",
    "oci",
    "ory",
    "press",
    "stalwart",
    "storage",
    "vault-secrets-operator",
    "vpn",
    "wfe",
];

/// Tear down all Sunbeam-managed namespaces.
///
/// * `infra` — also delete cert-manager and longhorn-system.
/// * `keep_data` — preserve the data namespace (postgres, opensearch, openbao).
#[tracing::instrument]
pub async fn cmd_down(yes: bool, infra: bool, keep_data: bool) -> Result<()> {
    tracing::debug!("cmd_down infra={infra} keep_data={keep_data}");
    let mut to_delete: Vec<&str> = APP_NAMESPACES.to_vec();

    if infra {
        to_delete.extend(INFRA_NAMESPACES);
    }

    if keep_data {
        to_delete.retain(|&ns| ns != "data");
    }

    // Filter to namespaces that actually exist on the cluster
    let client = crate::kube::get_client().await?;
    let ns_api: kube::api::Api<k8s_openapi::api::core::v1::Namespace> =
        kube::api::Api::all(client.clone());
    let existing = ns_api.list(&kube::api::ListParams::default()).await?;
    let existing_names: std::collections::HashSet<String> = existing
        .items
        .into_iter()
        .filter_map(|n| n.metadata.name)
        .collect();

    to_delete.retain(|ns| existing_names.contains(*ns));

    if to_delete.is_empty() {
        tracing::info!("No Sunbeam-managed namespaces found — nothing to delete.");
        return Ok(());
    }

    tracing::info!(
        "The following namespaces will be deleted:\n  {}",
        to_delete.join("\n  ")
    );

    if !yes {
        eprint!("\nProceed? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Delete in reverse dependency order:
    // apps first (they depend on infra), then infra
    // Within apps: no strict order needed, but we batch them
    let dp = kube::api::DeleteParams::background();

    for ns in &to_delete {
        tracing::info!("Deleting namespace {ns}...");
        match ns_api.delete(*ns, &dp).await {
            Ok(_) => {
                tracing::info!("  {ns} deletion started.");
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                tracing::info!("  {ns} already gone.");
            }
            Err(e) => {
                tracing::warn!("  Failed to delete {ns}: {e}");
            }
        }
    }

    // Wait for namespaces to actually terminate
    tracing::info!("Waiting for namespaces to terminate...");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut remaining = Vec::new();
    loop {
        if std::time::Instant::now() > deadline {
            tracing::warn!("Timed out waiting for namespace deletion.");
            break;
        }
        remaining.clear();
        for &ns in &to_delete {
            if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                remaining.push(ns.to_string());
            }
        }
        if remaining.is_empty() {
            tracing::info!("All namespaces deleted.");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }

    // ── Force-delete stuck namespaces ────────────────────────────────────────
    // Namespaces with finalizers (VaultAuth, VaultDynamicSecret, Longhorn
    // volumes, etc.) can get stuck in Terminating. Remove finalizers from
    // resources inside the namespace, then from the namespace itself.
    for ns in &remaining {
        tracing::info!("Force-deleting stuck namespace {ns}...");
        if let Err(e) = force_delete_namespace(client.clone(), ns).await {
            tracing::warn!("  Force-delete failed for {ns}: {e}");
        }
    }

    // Brief wait after force-delete
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let mut still_stuck = Vec::new();
        for ns in &remaining {
            if ns_api.get_opt(ns).await.ok().flatten().is_some() {
                still_stuck.push(ns.clone());
            }
        }
        if still_stuck.is_empty() {
            tracing::info!("All namespaces deleted after force-delete.");
            return Ok(());
        }
        remaining = still_stuck;
    }

    if !remaining.is_empty() {
        tracing::warn!(
            "Namespaces still stuck after force-delete: {}",
            remaining.join(", ")
        );
    }

    Ok(())
}

/// Remove finalizers from all namespaced resources in `namespace`, then
/// remove the namespace's own finalizers.
/// Remove finalizers from all namespaced resources in `namespace`, then
/// remove the namespace's own finalizers.
#[tracing::instrument(skip(client))]
pub async fn force_delete_namespace(client: kube::Client, namespace: &str) -> Result<()> {
    use kube::api::{Api, DynamicObject, Patch, PatchParams, ResourceExt};
    use kube::discovery::Scope;

    let pp = PatchParams::apply("sunbeam-force-delete").force();

    // Discover all namespaced API resources
    let disc = match kube::discovery::Discovery::new(client.clone()).run().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                "  Discovery failed, falling back to namespace finalizer removal only: {e}"
            );
            return remove_namespace_finalizers(client, namespace).await;
        }
    };

    // Remove finalizers from every namespaced resource
    for api_group in disc.groups() {
        for (ar, caps) in api_group.resources_by_stability() {
            if caps.scope != Scope::Namespaced {
                continue;
            }
            let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
            match api.list(&kube::api::ListParams::default().limit(500)).await {
                Ok(list) => {
                    for obj in list {
                        let name = obj.name_any();
                        if obj
                            .metadata
                            .finalizers
                            .as_ref()
                            .map(|f| f.is_empty())
                            .unwrap_or(true)
                        {
                            continue;
                        }
                        let patch = serde_json::json!({
                            "metadata": {
                                "finalizers": null
                            }
                        });
                        if let Err(e) = api.patch(&name, &pp, &Patch::Merge(&patch)).await {
                            tracing::warn!(
                                "    Could not patch finalizers on {}/{}: {}",
                                ar.kind, name, e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "    Could not list {} in {namespace}: {e}",
                        ar.kind
                    );
                }
            }
        }
    }

    // Finally remove the namespace's own finalizers
    remove_namespace_finalizers(client, namespace).await
}

/// Patch a namespace to remove its `spec.finalizers`.
async fn remove_namespace_finalizers(client: kube::Client, namespace: &str) -> Result<()> {
    let ns_api: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client);
    let patch = serde_json::json!({
        "metadata": {
            "finalizers": []
        }
    });
    let pp = PatchParams::apply("sunbeam-force-delete").force();
    ns_api
        .patch(namespace, &pp, &Patch::Merge(&patch))
        .await
        .map_err(|e| {
            crate::error::SunbeamError::kube(format!("Failed to patch namespace finalizers: {e}"))
        })?;
    Ok(())
}
