//! Native `kubectl describe deployment` replacement.
//!
//! Fetches a Deployment via kube-rs, its owned ReplicaSets, and matching Pods,
//! and formats a short summary. Nowhere near as detailed as `kubectl describe`,
//! but covers the common case for `sunbeam service describe`.

use sdk::k8s_openapi;
use sdk::kube_rs as kube;

use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
use k8s_openapi::api::core::v1::Pod;
use kube::Client;
use kube::api::{Api, ListParams};
use sdk::error::{Result, SunbeamError};
use std::fmt::Write as _;

/// Fetch a Deployment and render a describe-style summary.
#[tracing::instrument(skip(client))]
pub async fn describe_deployment(client: Client, ns: &str, name: &str) -> Result<String> {
    let deploys: Api<Deployment> = Api::namespaced(client.clone(), ns);
    let rss: Api<ReplicaSet> = Api::namespaced(client.clone(), ns);
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);

    let dep = deploys
        .get(name)
        .await
        .map_err(|e| SunbeamError::Other(format!("failed to get deployment {ns}/{name}: {e}")))?;

    let uid = dep.metadata.uid.clone().unwrap_or_default();

    let mut out = String::new();
    writeln!(out, "Name:           {name}").ok();
    writeln!(out, "Namespace:      {ns}").ok();

    let spec = dep.spec.as_ref();
    let status = dep.status.as_ref();

    let desired = spec.and_then(|s| s.replicas).unwrap_or(0);
    let available = status.and_then(|s| s.available_replicas).unwrap_or(0);
    let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
    let updated = status.and_then(|s| s.updated_replicas).unwrap_or(0);
    writeln!(
        out,
        "Replicas:       {desired} desired / {available} available ({ready} ready, {updated} updated)"
    )
    .ok();

    if let Some(strategy) = spec.and_then(|s| s.strategy.as_ref())
        && let Some(ty) = strategy.type_.as_deref()
    {
        writeln!(out, "Strategy:       {ty}").ok();
    }

    // Container images (first container is the primary in the usual case).
    if let Some(tpl) = spec.and_then(|s| s.template.spec.as_ref()) {
        for c in &tpl.containers {
            let img = c.image.as_deref().unwrap_or("<none>");
            writeln!(out, "Image:          {}  ({img})", c.name).ok();
        }
    }

    // Conditions.
    if let Some(conds) = status.and_then(|s| s.conditions.as_ref())
        && !conds.is_empty()
    {
        writeln!(out, "Conditions:").ok();
        for c in conds {
            let msg = c.message.as_deref().unwrap_or("");
            writeln!(out, "  {} = {} — {}", c.type_, c.status, msg).ok();
        }
    }

    // Owned ReplicaSets by ownerReference UID match.
    let lp = ListParams::default();
    let rs_list = rss.list(&lp).await.ok();
    let mut latest_rs: Option<ReplicaSet> = None;
    if let Some(rsl) = &rs_list {
        for rs in &rsl.items {
            let owned = rs
                .metadata
                .owner_references
                .as_ref()
                .map(|ors| ors.iter().any(|o| o.uid == uid))
                .unwrap_or(false);
            if !owned {
                continue;
            }
            let replicas = rs.status.as_ref().map(|s| s.replicas).unwrap_or(0);
            if replicas > 0 {
                latest_rs = Some(rs.clone());
                break;
            }
        }
    }

    // Pods matching the deployment's selector.
    if let Some(selector) = spec.and_then(|s| {
        s.selector.match_labels.as_ref().map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
    }) && !selector.is_empty()
    {
        let lp = ListParams::default().labels(&selector);
        if let Ok(pod_list) = pods.list(&lp).await
            && !pod_list.items.is_empty()
        {
            writeln!(out, "Pods:").ok();
            for p in &pod_list.items {
                let pname = p.metadata.name.as_deref().unwrap_or("<?>");
                let phase = p
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.as_deref())
                    .unwrap_or("?");
                let restarts: i32 = p
                    .status
                    .as_ref()
                    .and_then(|s| s.container_statuses.as_ref())
                    .map(|cs| cs.iter().map(|c| c.restart_count).sum())
                    .unwrap_or(0);
                let age = p
                    .metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|t| format_age(&t.0))
                    .unwrap_or_else(|| "?".into());
                writeln!(out, "  {pname} — {phase} — {restarts}r — {age}").ok();
            }
        }
    }

    // Latest ReplicaSet hint.
    if let Some(rs) = latest_rs
        && let Some(n) = rs.metadata.name.as_deref()
    {
        writeln!(out, "Active ReplicaSet: {n}").ok();
    }

    Ok(out)
}

fn format_age(t: &jiff::Timestamp) -> String {
    let secs = jiff::Timestamp::now().duration_since(*t).as_secs().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age_of(t: &jiff::Timestamp) -> String {
        format_age(t)
    }

    #[test]
    fn format_age_seconds() {
        let now = jiff::Timestamp::now();
        assert!(age_of(&now).ends_with('s'));
    }

    #[test]
    fn format_age_minutes_hours_days() {
        let now = jiff::Timestamp::now();
        let ten_min = now
            .checked_sub(jiff::SignedDuration::from_secs(600))
            .unwrap();
        assert_eq!(age_of(&ten_min), "10m");
        let two_hours = now
            .checked_sub(jiff::SignedDuration::from_secs(7200))
            .unwrap();
        assert_eq!(age_of(&two_hours), "2h");
        let three_days = now
            .checked_sub(jiff::SignedDuration::from_secs(3 * 86400))
            .unwrap();
        assert_eq!(age_of(&three_days), "3d");
    }

    #[test]
    fn format_age_future_clamps_to_zero() {
        let future = jiff::Timestamp::now()
            .checked_add(jiff::SignedDuration::from_secs(3600))
            .unwrap();
        assert_eq!(age_of(&future), "0s");
    }
}
