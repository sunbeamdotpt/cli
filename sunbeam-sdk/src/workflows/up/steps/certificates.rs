//! Certificate steps: TLS cert generation, TLS secret, cert-manager install.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::{ok, step};
use crate::workflows::data::UpData;

fn secrets_dir(context_name: &str) -> std::path::PathBuf {
    let name = if context_name.is_empty() {
        "default"
    } else {
        context_name
    };
    crate::config::context_dir(name).join("secrets")
}

// ── EnsureTLSCert ───────────────────────────────────────────────────────────

/// Generate a self-signed wildcard TLS certificate if one doesn't exist.
#[derive(Default)]
pub struct EnsureTLSCert;

#[async_trait::async_trait]
impl StepBody for EnsureTLSCert {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        let domain = resolve_domain(&data)?;

        step("TLS certificate...");

        let ctx_name = data
            .ctx
            .as_ref()
            .map(|c| c.context_name.as_str())
            .unwrap_or("default");
        let dir = secrets_dir(ctx_name);
        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");

        if cert_path.exists() {
            // Verify the existing cert is for the current domain
            let cert_pem = std::fs::read_to_string(&cert_path).map_err(|e| {
                wfe_core::WfeError::StepExecution(format!("Failed to read existing cert: {e}"))
            })?;
            let cert_for_domain = cert_pem.contains(&format!("*.{domain}"));
            if cert_for_domain {
                ok(&format!("Cert exists. Domain: {domain}"));
                return Ok(ExecutionResult::next());
            }
            ok(&format!(
                "Existing cert is for a different domain — regenerating for *.{domain}..."
            ));
        } else {
            ok(&format!("Generating wildcard cert for *.{domain}..."));
        }
        std::fs::create_dir_all(&dir).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!(
                "Failed to create secrets dir {}: {e}",
                dir.display()
            ))
        })?;

        let subject_alt_names = vec![format!("*.{domain}")];
        let mut params = rcgen::CertificateParams::new(subject_alt_names).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to create certificate params: {e}"))
        })?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, format!("*.{domain}"));

        let key_pair = rcgen::KeyPair::generate().map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to generate key pair: {e}"))
        })?;
        let cert = params.self_signed(&key_pair).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!(
                "Failed to generate self-signed certificate: {e}"
            ))
        })?;

        std::fs::write(&cert_path, cert.pem()).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!(
                "Failed to write {}: {e}",
                cert_path.display()
            ))
        })?;
        std::fs::write(&key_path, key_pair.serialize_pem()).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!(
                "Failed to write {}: {e}",
                key_path.display()
            ))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| {
                    wfe_core::WfeError::StepExecution(format!("Failed to set key permissions: {e}"))
                },
            )?;
        }

        ok(&format!("Cert generated. Domain: {domain}"));

        // Ensure Docker allows pushing to the local registry without cert
        // validation (self-signed wildcard cert).
        configure_docker_insecure_registries(&domain);

        Ok(ExecutionResult::next())
    }
}

/// Add src.<domain> and oci.<domain> to Docker's insecure-registries list
/// so `docker buildx --push` works against the local zot registry.
fn configure_docker_insecure_registries(domain: &str) {
    let daemon_path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
        .join(".docker/daemon.json");

    let mut cfg: serde_json::Map<String, serde_json::Value> =
        std::fs::read_to_string(&daemon_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

    let registries = vec![format!("src.{domain}"), format!("oci.{domain}")];
    let mut updated = false;

    let existing: Vec<String> = cfg
        .get("insecure-registries")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut new_list: Vec<serde_json::Value> =
        existing.iter().map(|s| serde_json::Value::String(s.clone())).collect();

    for reg in &registries {
        if !existing.iter().any(|e| e == reg) {
            new_list.push(serde_json::Value::String(reg.clone()));
            updated = true;
        }
    }

    if updated {
        cfg.insert(
            "insecure-registries".to_string(),
            serde_json::Value::Array(new_list),
        );
        if let Err(e) = (|| -> std::io::Result<()> {
            let tmp = daemon_path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(&cfg)?)?;
            std::fs::rename(&tmp, &daemon_path)?;
            Ok(())
        })() {
            crate::output::warn(&format!(
                "Failed to update Docker daemon.json for insecure registries: {e}\n\
                 You may need to manually add {registries:?} to ~/.docker/daemon.json -> insecure-registries"
            ));
        } else {
            crate::output::ok(&format!(
                "Docker insecure-registries updated: {registries:?}. \
                 Restart Docker Desktop for changes to take effect."
            ));
        }
    }
}

// ── EnsureTLSSecret ─────────────────────────────────────────────────────────

/// Apply the TLS secret to the ingress namespace.
#[derive(Default)]
pub struct EnsureTLSSecret;

#[async_trait::async_trait]
impl StepBody for EnsureTLSSecret {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        step("TLS secret...");

        k::ensure_ns("ingress")
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        let ctx_name = data
            .ctx
            .as_ref()
            .map(|c| c.context_name.as_str())
            .unwrap_or("default");
        let dir = secrets_dir(ctx_name);
        let cert_pem = std::fs::read_to_string(dir.join("tls.crt")).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to read tls.crt: {e}"))
        })?;
        let key_pem = std::fs::read_to_string(dir.join("tls.key")).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to read tls.key: {e}"))
        })?;

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let b64_cert = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            cert_pem.as_bytes(),
        );
        let b64_key = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            key_pem.as_bytes(),
        );

        let pp = kube::api::PatchParams::apply("sunbeam").force();

        // Ingress TLS secret
        let ingress_api: kube::api::Api<k8s_openapi::api::core::v1::Secret> =
            kube::api::Api::namespaced(client.clone(), "ingress");
        let ingress_secret = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "pingora-tls",
                "namespace": "ingress",
            },
            "type": "kubernetes.io/tls",
            "data": {
                "tls.crt": &b64_cert,
                "tls.key": &b64_key,
            },
        });
        ingress_api
            .patch("pingora-tls", &pp, &kube::api::Patch::Apply(ingress_secret))
            .await
            .map_err(|e| {
                wfe_core::WfeError::StepExecution(format!(
                    "Failed to create ingress TLS secret: {e}"
                ))
            })?;

        // Headscale TLS secret (same wildcard cert covers vpn.{domain})
        k::ensure_ns("vpn")
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let vpn_api: kube::api::Api<k8s_openapi::api::core::v1::Secret> =
            kube::api::Api::namespaced(client.clone(), "vpn");
        let vpn_secret = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "headscale-tls",
                "namespace": "vpn",
            },
            "type": "kubernetes.io/tls",
            "data": {
                "tls.crt": &b64_cert,
                "tls.key": &b64_key,
            },
        });
        vpn_api
            .patch("headscale-tls", &pp, &kube::api::Patch::Apply(vpn_secret))
            .await
            .map_err(|e| {
                wfe_core::WfeError::StepExecution(format!(
                    "Failed to create headscale TLS secret: {e}"
                ))
            })?;

        ok("Done.");
        Ok(ExecutionResult::next())
    }
}

// ── WaitForCertManagerWebhook ───────────────────────────────────────────────

/// Wait for cert-manager webhook to be fully ready.
///
/// On a fresh install, cainjector must inject the CA bundle into the
/// validating webhook before any `Certificate` resources can be created.
/// This step waits for:
/// 1. cert-manager, webhook, and cainjector deployments to be ready
/// 2. The `v1.cert-manager.io` APIService to report `Available=True`
#[derive(Default)]
pub struct WaitForCertManagerWebhook;

#[async_trait::async_trait]
impl StepBody for WaitForCertManagerWebhook {
    async fn run(&mut self, _ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        use k8s_openapi::api::apps::v1::Deployment;
        use k8s_openapi::kube_aggregator::pkg::apis::apiregistration::v1::APIService;
        use kube::api::{Api, ListParams};
        use std::time::{Duration, Instant};

        step("Waiting for cert-manager webhook...");

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let deploy_api: Api<Deployment> = Api::namespaced(client.clone(), "cert-manager");
        let apiservice_api: Api<APIService> = Api::all(client.clone());

        let deadline = Instant::now() + Duration::from_secs(120);
        let deployments = &[
            "cert-manager",
            "cert-manager-webhook",
            "cert-manager-cainjector",
        ];

        loop {
            if Instant::now() > deadline {
                return Err(wfe_core::WfeError::StepExecution(
                    "Timed out waiting for cert-manager webhook to be ready".into(),
                ));
            }

            let mut all_ready = true;

            for name in deployments {
                match deploy_api.get_opt(name).await {
                    Ok(Some(dep)) => {
                        let ready = dep
                            .status
                            .as_ref()
                            .and_then(|s| s.conditions.as_ref())
                            .map_or(false, |conds| {
                                conds
                                    .iter()
                                    .any(|c| c.type_ == "Available" && c.status == "True")
                            });
                        if !ready {
                            all_ready = false;
                            break;
                        }
                    }
                    Ok(None) => {
                        all_ready = false;
                        break;
                    }
                    Err(e) => {
                        return Err(wfe_core::WfeError::StepExecution(format!(
                            "Failed to get deployment cert-manager/{name}: {e}"
                        )));
                    }
                }
            }

            if all_ready {
                // Check APIService availability
                match apiservice_api.get_opt("v1.cert-manager.io").await {
                    Ok(Some(svc)) => {
                        let available = svc
                            .status
                            .as_ref()
                            .and_then(|s| s.conditions.as_ref())
                            .map_or(false, |conds| {
                                conds
                                    .iter()
                                    .any(|c| c.type_ == "Available" && c.status == "True")
                            });
                        if available {
                            ok("cert-manager webhook is ready.");
                            return Ok(ExecutionResult::next());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Err(wfe_core::WfeError::StepExecution(format!(
                            "Failed to get APIService v1.cert-manager.io: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn resolve_domain(data: &UpData) -> wfe_core::Result<String> {
    if !data.domain.is_empty() {
        return Ok(data.domain.clone());
    }
    if let Some(ctx) = &data.ctx
        && !ctx.domain.is_empty()
    {
        return Ok(ctx.domain.clone());
    }
    Err(wfe_core::WfeError::StepExecution(
        "domain not resolved".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_dir_uses_context_name() {
        let dir = secrets_dir("test-ctx");
        let s = dir.to_string_lossy();
        assert!(
            s.contains("test-ctx") && s.contains("secrets"),
            "secrets_dir should contain context name and 'secrets', got: {}",
            dir.display()
        );
    }

    #[test]
    fn secrets_dir_defaults_to_default_context() {
        let dir = secrets_dir("");
        let s = dir.to_string_lossy();
        assert!(
            s.contains("default") && s.contains("secrets"),
            "secrets_dir('') should default to 'default' context, got: {}",
            dir.display()
        );
    }

    #[test]
    fn ensure_tls_cert_is_default() {
        let _ = EnsureTLSCert;
    }

    #[test]
    fn ensure_tls_secret_is_default() {
        let _ = EnsureTLSSecret;
    }
}
