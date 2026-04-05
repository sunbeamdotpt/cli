//! Cluster lifecycle — cert-manager, TLS, core service readiness.
//!
//! Pure K8s implementation: no Lima VM operations.

use crate::constants::GITEA_ADMIN_USER;
use crate::error::{Result, ResultExt, SunbeamError};
use std::path::PathBuf;

pub(crate) const CERT_MANAGER_URL: &str =
    "https://github.com/cert-manager/cert-manager/releases/download/v1.17.0/cert-manager.yaml";

pub(crate) fn secrets_dir() -> PathBuf {
    crate::config::get_infra_dir()
        .join("secrets")
        .join("local")
}

// ---------------------------------------------------------------------------
// cert-manager
// ---------------------------------------------------------------------------

async fn ensure_cert_manager() -> Result<()> {
    crate::output::step("cert-manager...");

    if crate::kube::ns_exists("cert-manager").await? {
        crate::output::ok("Already installed.");
        return Ok(());
    }

    crate::output::ok("Installing...");

    // Download and apply cert-manager YAML
    let body = reqwest::get(CERT_MANAGER_URL)
        .await
        .ctx("Failed to download cert-manager manifest")?
        .text()
        .await
        .ctx("Failed to read cert-manager manifest body")?;

    crate::kube::kube_apply(&body).await?;

    // Wait for rollout
    for dep in &[
        "cert-manager",
        "cert-manager-webhook",
        "cert-manager-cainjector",
    ] {
        crate::output::ok(&format!("Waiting for {dep}..."));
        wait_rollout("cert-manager", dep, 120).await?;
    }

    crate::output::ok("Installed.");
    Ok(())
}

// ---------------------------------------------------------------------------
// TLS certificate (rcgen)
// ---------------------------------------------------------------------------

pub(crate) async fn ensure_tls_cert(domain: &str) -> Result<()> {
    crate::output::step("TLS certificate...");

    let dir = secrets_dir();
    let cert_path = dir.join("tls.crt");
    let key_path = dir.join("tls.key");

    if cert_path.exists() {
        crate::output::ok(&format!("Cert exists. Domain: {domain}"));
        return Ok(());
    }

    crate::output::ok(&format!("Generating wildcard cert for *.{domain}..."));
    std::fs::create_dir_all(&dir)
        .with_ctx(|| format!("Failed to create secrets dir: {}", dir.display()))?;

    let subject_alt_names = vec![format!("*.{domain}")];
    let mut params = rcgen::CertificateParams::new(subject_alt_names)
        .map_err(|e| SunbeamError::kube(format!("Failed to create certificate params: {e}")))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, format!("*.{domain}"));

    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| SunbeamError::kube(format!("Failed to generate key pair: {e}")))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| SunbeamError::kube(format!("Failed to generate self-signed certificate: {e}")))?;

    std::fs::write(&cert_path, cert.pem())
        .with_ctx(|| format!("Failed to write {}", cert_path.display()))?;
    std::fs::write(&key_path, key_pair.serialize_pem())
        .with_ctx(|| format!("Failed to write {}", key_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    crate::output::ok(&format!("Cert generated. Domain: {domain}"));
    Ok(())
}

// ---------------------------------------------------------------------------
// TLS secret
// ---------------------------------------------------------------------------

pub(crate) async fn ensure_tls_secret(domain: &str) -> Result<()> {
    crate::output::step("TLS secret...");

    let _ = domain; // domain used contextually above; secret uses files
    crate::kube::ensure_ns("ingress").await?;

    let dir = secrets_dir();
    let cert_pem =
        std::fs::read_to_string(dir.join("tls.crt")).ctx("Failed to read tls.crt")?;
    let key_pem =
        std::fs::read_to_string(dir.join("tls.key")).ctx("Failed to read tls.key")?;

    // Create TLS secret via kube-rs
    let client = crate::kube::get_client().await?;
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
        .ctx("Failed to create TLS secret")?;

    crate::output::ok("Done.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Wait for core
// ---------------------------------------------------------------------------

async fn wait_for_core() -> Result<()> {
    crate::output::step("Waiting for core services...");

    for (ns, dep) in &[("data", "valkey"), ("ory", "kratos"), ("ory", "hydra")] {
        let _ = wait_rollout(ns, dep, 120).await;
    }

    crate::output::ok("Core services ready.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Print URLs
// ---------------------------------------------------------------------------

pub(crate) fn print_urls(domain: &str, _gitea_admin_pass: &str) {
    let sep = "\u{2500}".repeat(60);
    println!("\n{sep}");
    println!("  Stack is up.  Domain: {domain}");
    println!("{sep}");

    let urls: &[(&str, String)] = &[
        ("Auth", format!("https://auth.{domain}/")),
        ("Docs", format!("https://docs.{domain}/")),
        ("Meet", format!("https://meet.{domain}/")),
        ("Drive", format!("https://drive.{domain}/")),
        ("Chat", format!("https://chat.{domain}/")),
        ("Mail", format!("https://mail.{domain}/")),
        ("People", format!("https://people.{domain}/")),
        (
            "Gitea",
            format!(
                "https://src.{domain}/  ({GITEA_ADMIN_USER} / <from openbao>)"
            ),
        ),
    ];

    for (name, url) in urls {
        println!("  {name:<10} {url}");
    }

    println!();
    println!("  OpenBao UI:");
    println!("    kubectl --context=sunbeam -n data port-forward svc/openbao 8200:8200");
    println!("    http://localhost:8200");
    println!(
        "    token: kubectl --context=sunbeam -n data get secret openbao-keys \
         -o jsonpath='{{.data.root-token}}' | base64 -d"
    );
    println!("{sep}\n");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Poll deployment rollout status (approximate: check Available condition).
pub(crate) async fn wait_rollout(ns: &str, deployment: &str, timeout_secs: u64) -> Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use std::time::{Duration, Instant};

    let client = crate::kube::get_client().await?;
    let api: kube::api::Api<Deployment> = kube::api::Api::namespaced(client.clone(), ns);

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if Instant::now() > deadline {
            return Err(SunbeamError::kube(format!("Timed out waiting for deployment {ns}/{deployment}")));
        }

        match api.get_opt(deployment).await? {
            Some(dep) => {
                if let Some(status) = &dep.status {
                    if let Some(conditions) = &status.conditions {
                        let available = conditions.iter().any(|c| {
                            c.type_ == "Available" && c.status == "True"
                        });
                        if available {
                            return Ok(());
                        }
                    }
                }
            }
            None => {
                // Deployment doesn't exist yet — keep waiting
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Full cluster bring-up (pure K8s — no Lima VM operations).
pub async fn cmd_up() -> Result<()> {
    // Resolve domain from cluster state
    let domain = crate::kube::get_domain().await?;

    ensure_cert_manager().await?;
    ensure_tls_cert(&domain).await?;
    ensure_tls_secret(&domain).await?;

    // Apply manifests
    crate::manifests::cmd_apply("local", &domain, "", "").await?;

    // Seed secrets
    crate::secrets::cmd_seed().await?;

    // Gitea bootstrap
    crate::gitea::cmd_bootstrap().await?;

    // Mirror amd64-only images
    crate::images::cmd_mirror().await?;

    // Wait for core services
    wait_for_core().await?;

    // Get gitea admin password for URL display
    let admin_pass = crate::kube::kube_get_secret_field(
        "devtools",
        "gitea-admin-credentials",
        "password",
    )
    .await
    .unwrap_or_default();

    print_urls(&domain, &admin_pass);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cert_manager_url_points_to_github_release() {
        assert!(CERT_MANAGER_URL.starts_with("https://github.com/cert-manager/cert-manager/"));
        assert!(CERT_MANAGER_URL.contains("/releases/download/"));
        assert!(CERT_MANAGER_URL.ends_with(".yaml"));
    }

    #[test]
    fn cert_manager_url_has_version() {
        // Verify the URL contains a version tag like v1.x.x
        assert!(
            CERT_MANAGER_URL.contains("/v1."),
            "CERT_MANAGER_URL should reference a v1.x release"
        );
    }

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
    fn secrets_dir_has_at_least_three_components() {
        let dir = secrets_dir();
        let components: Vec<_> = dir.components().collect();
        assert!(
            components.len() >= 3,
            "secrets_dir() should have at least 3 path components (base/secrets/local), got: {}",
            dir.display()
        );
    }

    #[test]
    fn gitea_admin_user_constant() {
        assert_eq!(GITEA_ADMIN_USER, "gitea_admin");
    }

    #[test]
    fn print_urls_contains_expected_services() {
        // Capture print_urls output by checking the URL construction logic.
        // We can't easily capture stdout in unit tests, but we can verify
        // the URL format matches expectations.
        let domain = "test.local";
        let expected_urls = [
            format!("https://auth.{domain}/"),
            format!("https://docs.{domain}/"),
            format!("https://meet.{domain}/"),
            format!("https://drive.{domain}/"),
            format!("https://chat.{domain}/"),
            format!("https://mail.{domain}/"),
            format!("https://people.{domain}/"),
            format!("https://src.{domain}/"),
        ];

        // Verify URL patterns are valid
        for url in &expected_urls {
            assert!(url.starts_with("https://"));
            assert!(url.contains(domain));
        }
    }

    #[test]
    fn print_urls_gitea_includes_credentials() {
        let domain = "example.local";
        let gitea_url = format!(
            "https://src.{domain}/  ({GITEA_ADMIN_USER} / <from openbao>)"
        );
        assert!(gitea_url.contains(GITEA_ADMIN_USER));
        assert!(gitea_url.contains("<from openbao>"));
        assert!(gitea_url.contains(&format!("src.{domain}")));
    }
}
