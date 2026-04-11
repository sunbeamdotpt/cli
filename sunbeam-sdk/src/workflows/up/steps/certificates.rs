//! Certificate steps: TLS cert generation, TLS secret, cert-manager install.

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::kube as k;
use crate::output::{ok, step};
use crate::workflows::data::UpData;

fn secrets_dir() -> std::path::PathBuf {
    crate::config::get_infra_dir().join("secrets").join("local")
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

        let dir = secrets_dir();
        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");

        if cert_path.exists() {
            ok(&format!("Cert exists. Domain: {domain}"));
            return Ok(ExecutionResult::next());
        }

        ok(&format!("Generating wildcard cert for *.{domain}..."));
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
        Ok(ExecutionResult::next())
    }
}

// ── EnsureTLSSecret ─────────────────────────────────────────────────────────

/// Apply the TLS secret to the ingress namespace.
#[derive(Default)]
pub struct EnsureTLSSecret;

#[async_trait::async_trait]
impl StepBody for EnsureTLSSecret {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let _data: UpData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        step("TLS secret...");

        k::ensure_ns("ingress")
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        let dir = secrets_dir();
        let cert_pem = std::fs::read_to_string(dir.join("tls.crt")).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to read tls.crt: {e}"))
        })?;
        let key_pem = std::fs::read_to_string(dir.join("tls.key")).map_err(|e| {
            wfe_core::WfeError::StepExecution(format!("Failed to read tls.key: {e}"))
        })?;

        let client = k::get_client()
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let api: kube::api::Api<k8s_openapi::api::core::v1::Secret> =
            kube::api::Api::namespaced(client.clone(), "ingress");

        let b64_cert = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            cert_pem.as_bytes(),
        );
        let b64_key = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            key_pem.as_bytes(),
        );

        let secret_obj = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": "pingora-tls",
                "namespace": "ingress",
            },
            "type": "kubernetes.io/tls",
            "data": {
                "tls.crt": b64_cert,
                "tls.key": b64_key,
            },
        });

        let pp = kube::api::PatchParams::apply("sunbeam").force();
        api.patch("pingora-tls", &pp, &kube::api::Patch::Apply(secret_obj))
            .await
            .map_err(|e| {
                wfe_core::WfeError::StepExecution(format!("Failed to create TLS secret: {e}"))
            })?;

        ok("Done.");
        Ok(ExecutionResult::next())
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
    fn secrets_dir_ends_with_secrets_local() {
        let dir = secrets_dir();
        assert!(
            dir.ends_with("secrets/local"),
            "secrets_dir() should end with secrets/local, got: {}",
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
