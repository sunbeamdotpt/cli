//! Cluster lifecycle helpers.

use crate::error::{Result, SunbeamError};

/// Poll deployment rollout status (approximate: check Available condition).
pub(crate) async fn wait_rollout(ns: &str, deployment: &str, timeout_secs: u64) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use std::time::{Duration, Instant};

    let client = crate::kube::get_client().await?;
    let api: kube::api::Api<Deployment> = kube::api::Api::namespaced(client.clone(), ns);

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if Instant::now() > deadline {
            return Err(SunbeamError::kube(format!(
                "Timed out waiting for deployment {ns}/{deployment}"
            )));
        }

        match api.get_opt(deployment).await? {
            Some(dep) => {
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
            None => {}
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::GITEA_ADMIN_USER;

    #[test]
    fn gitea_admin_user_constant() {
        assert_eq!(GITEA_ADMIN_USER, "gitea_admin");
    }
}
