//! Proxy image management — build, push, node-level pre-pull, kustomization bump.
//!
//! The entry point for the CLI is `cmd_preseed_image`, which applies the
//! image-puller Job to the cluster and waits for it to complete.  The
//! kustomization bump (`bump_proxy_tag`) is a pure function and is tested
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

/// Replace the `newTag` value for the `sunbeam-proxy` image entry inside a
/// kustomization YAML string.  Returns the updated string.
///
/// The replacement is line-oriented: it finds the `newTag:` line that
/// immediately follows the `- name: sunbeam-proxy` stanza and rewrites it.
/// Returns an error when the stanza is not found.
pub fn bump_proxy_tag(kustomization: &str, new_tag: &str) -> Result<String> {
    let mut lines: Vec<&str> = kustomization.lines().collect();
    let mut found = false;

    for i in 0..lines.len() {
        if lines[i].trim() == "- name: sunbeam-proxy" {
            // Scan forward for the newTag: line within the same stanza.
            for j in (i + 1)..lines.len() {
                let trimmed = lines[j].trim();
                if trimmed.starts_with("newTag:") {
                    // Preserve the leading indentation.
                    let indent: &str = &lines[j][..lines[j].len() - lines[j].trim_start().len()];
                    // We can't mutate `lines` (it's `Vec<&str>`), so build a
                    // new owned vec.
                    let owned: String = {
                        let mut out = Vec::with_capacity(lines.len());
                        for (k, line) in lines.iter().enumerate() {
                            if k == j {
                                out.push(format!("{indent}newTag: {new_tag}"));
                            } else {
                                out.push((*line).to_string());
                            }
                        }
                        out.join("\n")
                    };
                    // Preserve a trailing newline if the original had one.
                    let result = if kustomization.ends_with('\n') {
                        format!("{owned}\n")
                    } else {
                        owned
                    };
                    return Ok(result);
                }
                // Stop if we hit another image stanza `- name:` or a blank
                // line outside the stanza — don't walk past the block.
                if trimmed.starts_with("- name:") {
                    break;
                }
            }
            found = true; // stanza found but no newTag: line
            break;
        }
    }

    if found {
        bail!("sunbeam-proxy image stanza found but has no newTag: line");
    }
    bail!("sunbeam-proxy image stanza not found in kustomization")
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

    const KUSTOMIZATION: &str = "\
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: ingress

resources:
  - namespace.yaml
  - pingora-deployment.yaml

images:
  - name: sunbeam-proxy
    newName: src.sunbeam.pt/studio/proxy
    newTag: 4bb17d7c
";

    #[test]
    fn bump_proxy_tag_updates_tag() {
        let result = bump_proxy_tag(KUSTOMIZATION, "deadbeef").unwrap();
        assert!(result.contains("newTag: deadbeef"), "result={result}");
        assert!(!result.contains("newTag: 4bb17d7c"), "old tag still present");
    }

    #[test]
    fn bump_proxy_tag_preserves_trailing_newline() {
        let result = bump_proxy_tag(KUSTOMIZATION, "deadbeef").unwrap();
        assert!(result.ends_with('\n'), "trailing newline lost");
    }

    #[test]
    fn bump_proxy_tag_preserves_other_fields() {
        let result = bump_proxy_tag(KUSTOMIZATION, "cafebabe").unwrap();
        assert!(result.contains("newName: src.sunbeam.pt/studio/proxy"));
        assert!(result.contains("namespace: ingress"));
        assert!(result.contains("- name: sunbeam-proxy"));
    }

    #[test]
    fn bump_proxy_tag_no_trailing_newline() {
        let input = "images:\n  - name: sunbeam-proxy\n    newTag: aaaaaaaa";
        let result = bump_proxy_tag(input, "bbbbbbbb").unwrap();
        assert!(!result.ends_with('\n'));
        assert!(result.contains("newTag: bbbbbbbb"));
    }

    #[test]
    fn bump_proxy_tag_stanza_missing_errors() {
        let input = "images:\n  - name: other-image\n    newTag: 1234\n";
        let err = bump_proxy_tag(input, "abcd1234").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "err={err}"
        );
    }

    #[test]
    fn bump_proxy_tag_multiple_images_only_updates_proxy() {
        let input = "\
images:
  - name: other-image
    newTag: 0000
  - name: sunbeam-proxy
    newName: src.sunbeam.pt/studio/proxy
    newTag: aaaaaaaa
  - name: yet-another
    newTag: 9999
";
        let result = bump_proxy_tag(input, "bbbbbbbb").unwrap();
        assert!(result.contains("newTag: 0000"), "other-image changed");
        assert!(result.contains("newTag: 9999"), "yet-another changed");
        assert!(result.contains("newTag: bbbbbbbb"), "proxy not updated");
        assert!(!result.contains("newTag: aaaaaaaa"), "old proxy tag remains");
    }
}
