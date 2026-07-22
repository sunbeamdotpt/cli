//! Kratos admin identity steps: seed admin identity, print outputs.
#![allow(dead_code)]

use std::collections::HashMap;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use sdk::error::SunbeamError;
use sdk::kube as k;
use sdk::openbao::BaoClient;

use crate::workflows::data::SeedData;
use sdk::reqwest;
use sdk::secrets::{self as secrets, ADMIN_USERNAME, KratosIdentity, KratosRecovery};

// ── Pure helpers (testable without K8s) ─────────────────────────────────────

/// Strip PEM header/footer lines and whitespace from a public key,
/// returning the raw base64 content.
pub(crate) fn strip_pem_headers(pem: &str) -> String {
    pem.replace("-----BEGIN PUBLIC KEY-----", "")
        .replace("-----END PUBLIC KEY-----", "")
        .replace("-----BEGIN RSA PUBLIC KEY-----", "")
        .replace("-----END RSA PUBLIC KEY-----", "")
        .split_whitespace()
        .collect()
}

/// Format a DKIM DNS TXT record from domain and base64-encoded public key.
pub(crate) fn format_dkim_record(domain: &str, b64_key: &str) -> String {
    format!("default._domainkey.{domain}  TXT  \"v=DKIM1; k=rsa; p={b64_key}\"")
}

/// Build the admin email from the domain.
pub(crate) fn admin_email(domain: &str) -> String {
    format!("{ADMIN_USERNAME}@{domain}")
}

// ── SeedKratosAdminIdentity ─────────────────────────────────────────────────

/// Port-forward to Kratos, check/create admin identity, generate recovery code,
/// update OpenBao.
#[derive(Default)]
pub struct SeedKratosAdminIdentity;

#[async_trait::async_trait]
impl StepBody for SeedKratosAdminIdentity {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: SeedData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        if data.skip_seed {
            return Ok(ExecutionResult::next());
        }
        if data.skip_ory {
            tracing::info!("Skipping Kratos admin identity seed (skip_ory=true)");
            return Ok(ExecutionResult::next());
        }
        let skip_namespaces: Vec<String> = ctx
            .workflow
            .data
            .get("skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if skip_namespaces.contains(&"ory".to_string()) {
            tracing::info!("Skipping Kratos admin identity seed (profile skip list)");
            return Ok(ExecutionResult::next());
        }

        let ob_pod = match &data.ob_pod {
            Some(p) => p.clone(),
            None => return Ok(ExecutionResult::next()),
        };
        let root_token = match &data.root_token {
            Some(t) if !t.is_empty() => t.clone(),
            _ => return Ok(ExecutionResult::next()),
        };

        let domain = match k::get_domain().await {
            Ok(d) => d,
            Err(e) => {
                return Err(wfe_core::WfeError::StepExecution(format!(
                    "Could not determine domain: {e}"
                )));
            }
        };
        let admin_email = admin_email(&domain);
        tracing::info!("Ensuring Kratos admin identity ({admin_email})...");

        let pf_bao = secrets::port_forward("data", &ob_pod, 8200)
            .await
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;
        let bao_url = format!("http://127.0.0.1:{}", pf_bao.local_port);
        let bao = BaoClient::with_token(&bao_url, &root_token);

        let result: std::result::Result<(String, String, String), SunbeamError> = async {
            let pf =
                match secrets::port_forward_svc("ory", "app.kubernetes.io/name=kratos-admin", 80)
                    .await
                {
                    Ok(pf) => pf,
                    Err(_) => {
                        secrets::port_forward_svc("ory", "app.kubernetes.io/name=kratos", 4434)
                            .await
                            .map_err(|e| {
                                SunbeamError::Other(format!(
                                    "Could not port-forward to Kratos admin API: {e}"
                                ))
                            })?
                    }
                };
            let base = format!("http://127.0.0.1:{}", pf.local_port);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            let http = reqwest::Client::new();

            let resp = http
                .get(format!(
                    "{base}/admin/identities?credentials_identifier={admin_email}&page_size=1"
                ))
                .header("Accept", "application/json")
                .send()
                .await?;

            let identities: Vec<KratosIdentity> = resp.json().await.unwrap_or_default();
            let identity_id = if let Some(existing) = identities.first() {
                tracing::info!(
                    "  admin identity exists ({}...)",
                    &existing.id[..8.min(existing.id.len())]
                );
                existing.id.clone()
            } else {
                let resp = http
                    .post(format!("{base}/admin/identities"))
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .json(&serde_json::json!({
                        "schema_id": "employee",
                        "traits": {"email": admin_email},
                        "state": "active",
                    }))
                    .send()
                    .await?;

                let identity: KratosIdentity = resp
                    .json()
                    .await
                    .map_err(|e| SunbeamError::Other(e.to_string()))?;
                tracing::info!(
                    "  created admin identity ({}...)",
                    &identity.id[..8.min(identity.id.len())]
                );
                identity.id
            };

            let resp = http
                .post(format!("{base}/admin/recovery/code"))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .json(&serde_json::json!({
                    "identity_id": identity_id,
                    "expires_in": "24h",
                }))
                .send()
                .await?;

            let recovery: KratosRecovery = resp.json().await.unwrap_or(KratosRecovery {
                recovery_link: String::new(),
                recovery_code: String::new(),
            });

            let mut patch_data = HashMap::new();
            patch_data.insert("admin-identity-ids".to_string(), admin_email.clone());
            let _ = bao.kv_patch("secret", "kratos-admin", &patch_data).await;
            tracing::info!("  ADMIN_IDENTITY_IDS set to {admin_email}");

            Ok((recovery.recovery_link, recovery.recovery_code, identity_id))
        }
        .await;

        let mut output = ExecutionResult::next();
        match result {
            Ok((recovery_link, recovery_code, identity_id)) => {
                output.output_data = Some(serde_json::json!({
                    "recovery_link": recovery_link,
                    "recovery_code": recovery_code,
                    "admin_identity_id": identity_id,
                }));
            }
            Err(e) => {
                return Err(wfe_core::WfeError::StepExecution(format!(
                    "Could not seed Kratos admin identity: {e}"
                )));
            }
        }

        Ok(output)
    }
}

// ── PrintSeedOutputs ────────────────────────────────────────────────────────

/// Print DKIM record and recovery link/code.
#[derive(Default)]
pub struct PrintSeedOutputs;

#[async_trait::async_trait]
impl StepBody for PrintSeedOutputs {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let data: SeedData = serde_json::from_value(ctx.workflow.data.clone())
            .map_err(|e| wfe_core::WfeError::StepExecution(e.to_string()))?;

        if data.skip_seed {
            tracing::info!("Seed skipped (OpenBao not available).");
            return Ok(ExecutionResult::next());
        }

        if let Some(ref link) = data.recovery_link
            && !link.is_empty()
        {
            tracing::info!("Admin recovery link (valid 24h):");
            println!("  {link}");
        }
        if let Some(ref code) = data.recovery_code
            && !code.is_empty()
        {
            tracing::info!("Admin recovery code (enter on the page above):");
            println!("  {code}");
        }

        let dkim_pub = data
            .creds
            .get("messages-dkim-public-key")
            .cloned()
            .unwrap_or_default();
        if !dkim_pub.is_empty() {
            let b64_key = strip_pem_headers(&dkim_pub);

            if let Ok(domain) = k::get_domain().await {
                tracing::info!("DKIM DNS record (add to DNS at your registrar):");
                println!("  {}", format_dkim_record(&domain, &b64_key));
            }
        }

        tracing::info!("All secrets seeded.");
        Ok(ExecutionResult::next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wfe::run_workflow_sync;
    use wfe_core::builder::WorkflowBuilder;
    use wfe_core::models::WorkflowStatus;

    async fn run_step<S: StepBody + Default + 'static>(
        data: serde_json::Value,
    ) -> wfe_core::models::WorkflowInstance {
        let host = crate::workflows::host::create_test_host().await.unwrap();
        host.register_step::<S>().await;
        let def = WorkflowBuilder::<serde_json::Value>::new()
            .start_with::<S>()
            .name("test-step")
            .end_workflow()
            .build("test-wf", 1);
        host.register_workflow_definition(def).await;
        let instance = run_workflow_sync(&host, "test-wf", 1, data, Duration::from_secs(5))
            .await
            .unwrap();
        host.stop().await;
        instance
    }

    // ── SeedKratosAdminIdentity ─────────────────────────────────────────

    #[tokio::test]
    async fn test_seed_kratos_admin_skip_seed() {
        let data = serde_json::json!({ "skip_seed": true });
        let instance = run_step::<SeedKratosAdminIdentity>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_seed_kratos_admin_no_ob_pod() {
        let data = serde_json::json!({ "skip_seed": false });
        let instance = run_step::<SeedKratosAdminIdentity>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_seed_kratos_admin_null_ob_pod() {
        let data = serde_json::json!({ "skip_seed": false, "ob_pod": null });
        let instance = run_step::<SeedKratosAdminIdentity>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_seed_kratos_admin_no_root_token() {
        let data = serde_json::json!({ "skip_seed": false, "ob_pod": "openbao-0" });
        let instance = run_step::<SeedKratosAdminIdentity>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_seed_kratos_admin_empty_root_token() {
        let data = serde_json::json!({
            "skip_seed": false,
            "ob_pod": "openbao-0",
            "root_token": "",
        });
        let instance = run_step::<SeedKratosAdminIdentity>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    // ── PrintSeedOutputs ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_print_seed_outputs_skip_seed() {
        let data = serde_json::json!({ "skip_seed": true });
        let instance = run_step::<PrintSeedOutputs>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_print_seed_outputs_no_recovery_link() {
        let data = serde_json::json!({ "skip_seed": false });
        let instance = run_step::<PrintSeedOutputs>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_print_seed_outputs_with_recovery_data() {
        let data = serde_json::json!({
            "skip_seed": false,
            "recovery_link": "https://login.test.local/self-service/recovery?flow=abc",
            "recovery_code": "123456",
        });
        let instance = run_step::<PrintSeedOutputs>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    #[tokio::test]
    async fn test_print_seed_outputs_with_empty_recovery() {
        let data = serde_json::json!({
            "skip_seed": false,
            "recovery_link": "",
            "recovery_code": "",
        });
        let instance = run_step::<PrintSeedOutputs>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    // Note: PrintSeedOutputs with a non-empty DKIM key calls k::get_domain()
    // which requires a live kube cluster. The DKIM stripping logic is tested
    // separately in test_dkim_key_stripping below.

    #[tokio::test]
    async fn test_print_seed_outputs_empty_dkim_key() {
        let data = serde_json::json!({
            "skip_seed": false,
            "creds": { "messages-dkim-public-key": "" },
        });
        let instance = run_step::<PrintSeedOutputs>(data).await;
        assert_eq!(instance.status, WorkflowStatus::Complete);
    }

    // ── Data deserialization ────────────────────────────────────────────

    #[test]
    fn test_seed_data_recovery_fields() {
        let json = serde_json::json!({
            "skip_seed": false,
            "recovery_link": "https://example.com/recovery",
            "recovery_code": "abc123",
            "admin_identity_id": "id-uuid-here",
        });
        let data: SeedData = serde_json::from_value(json).unwrap();
        assert_eq!(
            data.recovery_link.as_deref(),
            Some("https://example.com/recovery")
        );
        assert_eq!(data.recovery_code.as_deref(), Some("abc123"));
        assert_eq!(data.admin_identity_id.as_deref(), Some("id-uuid-here"));
    }

    #[test]
    fn test_seed_data_recovery_fields_default_none() {
        let json = serde_json::json!({ "skip_seed": false });
        let data: SeedData = serde_json::from_value(json).unwrap();
        assert!(data.recovery_link.is_none());
        assert!(data.recovery_code.is_none());
        assert!(data.admin_identity_id.is_none());
    }

    #[test]
    fn test_dkim_key_stripping() {
        let raw = "-----BEGIN PUBLIC KEY-----\nMIIBIjAN\n-----END PUBLIC KEY-----";
        let b64_key = strip_pem_headers(raw);
        assert_eq!(b64_key, "MIIBIjAN");
    }

    // ── Pure helper tests ─────────────────────────────────────────────────

    #[test]
    fn test_strip_pem_headers_standard() {
        let pem = "-----BEGIN PUBLIC KEY-----\nMIIBIjAN\nBgkqhki\n-----END PUBLIC KEY-----";
        assert_eq!(strip_pem_headers(pem), "MIIBIjANBgkqhki");
    }

    #[test]
    fn test_strip_pem_headers_rsa_variant() {
        let pem = "-----BEGIN RSA PUBLIC KEY-----\nABC123\n-----END RSA PUBLIC KEY-----";
        assert_eq!(strip_pem_headers(pem), "ABC123");
    }

    #[test]
    fn test_strip_pem_headers_no_headers() {
        assert_eq!(strip_pem_headers("MIIBIjAN"), "MIIBIjAN");
    }

    #[test]
    fn test_strip_pem_headers_empty() {
        assert_eq!(strip_pem_headers(""), "");
    }

    #[test]
    fn test_strip_pem_headers_multiline() {
        let pem = "-----BEGIN PUBLIC KEY-----\nAAAA\nBBBB\nCCCC\n-----END PUBLIC KEY-----\n";
        assert_eq!(strip_pem_headers(pem), "AAAABBBBCCCC");
    }

    #[test]
    fn test_format_dkim_record() {
        let record = format_dkim_record("sunbeam.pt", "MIIBIjAN");
        assert_eq!(
            record,
            "default._domainkey.sunbeam.pt  TXT  \"v=DKIM1; k=rsa; p=MIIBIjAN\""
        );
    }

    #[test]
    fn test_format_dkim_record_different_domain() {
        let record = format_dkim_record("example.com", "ABC123");
        assert!(record.contains("example.com"));
        assert!(record.contains("ABC123"));
        assert!(record.starts_with("default._domainkey."));
    }

    #[test]
    fn test_admin_email() {
        let email = admin_email("sunbeam.pt");
        assert_eq!(email, format!("{}@sunbeam.pt", ADMIN_USERNAME));
    }

    #[test]
    fn test_admin_email_different_domain() {
        let email = admin_email("example.com");
        assert!(email.ends_with("@example.com"));
        assert!(email.starts_with(ADMIN_USERNAME));
    }
}
