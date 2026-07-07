//! Cluster lifecycle helpers.

use crate::error::{Result, SunbeamError};
use crate::info;

/// Poll deployment rollout status (approximate: check Available condition).
pub(crate) async fn wait_rollout(
    logger: &crate::logger::Logger,
    ns: &str,
    deployment: &str,
    timeout_secs: u64,
) -> Result<()> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut consecutive_transient_errors = 0;

    loop {
        if Instant::now() > deadline {
            return Err(SunbeamError::kube(format!(
                "Timed out waiting for deployment {ns}/{deployment}"
            )));
        }

        match try_rollout_check(ns, deployment).await {
            Ok(true) => return Ok(()),
            Ok(false) => {
                consecutive_transient_errors = 0;
            }
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                let is_transient = msg.contains("sendrequest")
                    || msg.contains("connection refused")
                    || msg.contains("connect")
                    || msg.contains("timeout")
                    || msg.contains("reset by peer")
                    || msg.contains("broken pipe");
                if !is_transient {
                    return Err(e);
                }
                consecutive_transient_errors += 1;
                if consecutive_transient_errors >= 10 {
                    return Err(SunbeamError::kube(format!(
                        "Too many consecutive transient errors waiting for {ns}/{deployment}: {e}"
                    )));
                }
                info!(
                    logger,
                    "Transient error waiting for deployment, retrying in 3s...",
                    ns = ns,
                    deployment = deployment,
                    consecutive_transient_errors = consecutive_transient_errors,
                    error = e.to_string()
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn try_rollout_check(ns: &str, deployment: &str) -> Result<bool> {
    use k8s_openapi::api::apps::v1::Deployment;

    let client = crate::kube::get_client().await?;
    let api: kube::api::Api<Deployment> = kube::api::Api::namespaced(client.clone(), ns);

    if let Some(dep) = api.get_opt(deployment).await?
        && let Some(status) = &dep.status
        && let Some(conditions) = &status.conditions
    {
        let available = conditions
            .iter()
            .any(|c| c.type_ == "Available" && c.status == "True");
        return Ok(available);
    }

    Ok(false)
}
