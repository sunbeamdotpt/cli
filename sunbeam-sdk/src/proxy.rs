//! Proxy image management — build, push, node-level pre-pull, profile bump.
//!
//! The entry point for the CLI is `cmd_preseed_image`, which applies the
//! image-puller Job to the cluster and waits for it to complete.  The
//! profile bump (`bump_proxy_image`) is a pure function and is tested
//! independently.

use crate::error::{Result, ResultExt, SunbeamError};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render the image-puller Job template and apply it to the cluster, then
/// wait up to `timeout_secs` for the Job to complete.
///
/// The template file lives at
/// `<infra_dir>/base/build/image-puller-job.template.yaml` and uses the
/// literal string `IMAGE_REF` as the substitution target.
#[tracing::instrument]
pub async fn cmd_preseed_image(image_ref: &str, timeout_secs: u64) -> Result<()> {
    let infra_dir = crate::config::get_infra_dir();
    let template_path = infra_dir
        .join("base")
        .join("build")
        .join("image-puller-job.template.yaml");

    let template = std::fs::read_to_string(&template_path).map_err(|e| SunbeamError::Io {
        context: format!("reading {}", template_path.display()),
        source: e,
    })?;

    let manifest = template.replace("IMAGE_REF", image_ref);

    // Delete any previous run of this Job so the apply is idempotent.
    delete_puller_job_if_exists().await;

    tracing::info!("Applying image-puller Job for {image_ref}...");
    crate::kube::kube_apply(&manifest).await?;

    tracing::info!("Job applied — waiting for completion...");
    wait_for_job("build", "proxy-image-puller", timeout_secs).await?;

    tracing::info!("Node has pulled {image_ref}.");
    tracing::debug!("preseed_image completed in {timeout_secs}s");
    Ok(())
}

/// Replace the `image` value for the `pingora` Deployment rule inside a
/// profile YAML string.  Returns the updated string.
///
/// The replacement is line-oriented: it finds the `resource: pingora` rule
/// that belongs to `namespace: ingress` and `kind: Deployment`, then
/// updates or inserts the `image:` line within that rule block.
/// Returns an error when the rule is not found.
pub fn bump_proxy_image(profile: &str, new_image: &str) -> Result<String> {
    let lines: Vec<&str> = profile.lines().collect();

    for i in 0..lines.len() {
        if lines[i].trim() == "resource: pingora" {
            // Verify this is the ingress Deployment rule.
            let mut is_target = false;
            let mut kind_idx = None;

            for j in (i + 1)..lines.len() {
                let trimmed = lines[j].trim();
                // Stop at next rule or section boundary.
                if trimmed.starts_with('-') && !trimmed.starts_with("- name:") {
                    break;
                }
                if trimmed == "kind: Deployment" {
                    is_target = true;
                    kind_idx = Some(j);
                }
            }

            if !is_target {
                continue;
            }

            let kind_idx = kind_idx.unwrap();
            let indent = "    ";

            // Look for an existing image: line after kind: Deployment.
            let mut image_idx = None;
            for j in (kind_idx + 1)..lines.len() {
                let trimmed = lines[j].trim();
                if trimmed.starts_with('-') && !trimmed.starts_with("- name:") {
                    break;
                }
                if trimmed.starts_with("image:") {
                    image_idx = Some(j);
                    break;
                }
            }

            // Build the output.
            let mut out: Vec<String> = Vec::with_capacity(lines.len() + 1);
            for (k, line) in lines.iter().enumerate() {
                if Some(k) == image_idx {
                    out.push(format!("{indent}image: \"{new_image}\""));
                } else {
                    out.push((*line).to_string());
                }
            }

            // If no image: line found, insert after kind: Deployment.
            if image_idx.is_none() {
                out.insert(kind_idx + 1, format!("{indent}image: \"{new_image}\""));
            }

            let mut result = out.join("\n");
            if profile.ends_with('\n') {
                result.push('\n');
            }
            return Ok(result);
        }
    }

    bail!("pingora Deployment rule not found in profile")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Delete the proxy-image-puller Job if it exists (idempotent pre-clean).
async fn delete_puller_job_if_exists() {
    let client = match crate::kube::get_client().await {
        Ok(c) => c,
        Err(_) => return,
    };
    let jobs: kube::api::Api<k8s_openapi::api::batch::v1::Job> =
        kube::api::Api::namespaced(client, "build");
    let dp = kube::api::DeleteParams {
        grace_period_seconds: Some(0),
        propagation_policy: Some(kube::api::PropagationPolicy::Background),
        ..Default::default()
    };
    let _ = jobs.delete("proxy-image-puller", &dp).await;
    // Brief pause to let the API server process the deletion before we re-apply.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}

/// Poll until the Job reaches Succeeded (or fails / times out).
async fn wait_for_job(ns: &str, name: &str, timeout_secs: u64) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if std::time::Instant::now() > deadline {
            bail!("timed out waiting for Job {ns}/{name} after {timeout_secs}s");
        }

        let client = crate::kube::get_client().await?;
        let jobs: kube::api::Api<k8s_openapi::api::batch::v1::Job> =
            kube::api::Api::namespaced(client, ns);

        match jobs.get_opt(name).await.ctx("reading Job status")? {
            None => {
                bail!("Job {ns}/{name} disappeared before completing");
            }
            Some(job) => {
                let status = job.status.as_ref();
                let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
                let failed = status.and_then(|s| s.failed).unwrap_or(0);
                let backoff_limit = job.spec.as_ref().and_then(|s| s.backoff_limit).unwrap_or(3);

                if succeeded > 0 {
                    return Ok(());
                }
                if failed > backoff_limit {
                    bail!("Job {ns}/{name} exceeded backoffLimit ({backoff_limit} failures)");
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = "\
rules:
  -
    resource: pingora
    namespace: ingress
    kind: Deployment
    image: \"old-image\"
";

    #[test]
    fn bump_proxy_image_updates_image() {
        let result = bump_proxy_image(PROFILE, "new-image").unwrap();
        assert!(result.contains("image: \"new-image\""), "result={result}");
        assert!(!result.contains("image: \"old-image\""), "old image still present");
    }

    #[test]
    fn bump_proxy_image_preserves_trailing_newline() {
        let result = bump_proxy_image(PROFILE, "new-image").unwrap();
        assert!(result.ends_with('\n'), "trailing newline lost");
    }

    #[test]
    fn bump_proxy_image_preserves_other_fields() {
        let result = bump_proxy_image(PROFILE, "new-image").unwrap();
        assert!(result.contains("resource: pingora"));
        assert!(result.contains("namespace: ingress"));
        assert!(result.contains("kind: Deployment"));
    }

    #[test]
    fn bump_proxy_image_inserts_image_when_missing() {
        let input = "\
rules:
  -
    resource: pingora
    namespace: ingress
    kind: Deployment
";
        let result = bump_proxy_image(input, "inserted-image").unwrap();
        assert!(result.contains("image: \"inserted-image\""), "result={result}");
    }

    #[test]
    fn bump_proxy_image_missing_rule_errors() {
        let input = "\
rules:
  -
    resource: other
    namespace: default
    kind: Deployment
";
        let err = bump_proxy_image(input, "abcd1234").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "err={err}"
        );
    }

    #[test]
    fn bump_proxy_image_multiple_rules_only_updates_pingora() {
        let input = "\
rules:
  -
    resource: other
    namespace: default
    kind: Deployment
    image: \"other-image\"
  -
    resource: pingora
    namespace: ingress
    kind: Deployment
    image: \"old-proxy-image\"
  -
    resource: another
    namespace: default
    kind: Deployment
    image: \"another-image\"
";
        let result = bump_proxy_image(input, "new-proxy-image").unwrap();
        assert!(result.contains("image: \"other-image\""), "other-image changed");
        assert!(result.contains("image: \"another-image\""), "another-image changed");
        assert!(result.contains("image: \"new-proxy-image\""), "proxy not updated");
        assert!(!result.contains("image: \"old-proxy-image\""), "old proxy image remains");
    }
}
