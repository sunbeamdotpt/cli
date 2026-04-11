//! Verify workflow definition — VSO ↔ OpenBao E2E verification.

use wfe_core::builder::WorkflowBuilder;
use wfe_core::models::WorkflowDefinition;

use super::steps;

/// Build the verify workflow definition.
///
/// Steps execute sequentially:
/// 1. Find OpenBao pod
/// 2. Get root token from K8s secret
/// 3. Write sentinel value to OpenBao
/// 4. Apply VaultAuth CRD
/// 5. Apply VaultStaticSecret CRD
/// 6. Wait for VSO to sync
/// 7. Check K8s Secret value matches sentinel
/// 8. Clean up test resources
/// 9. Print result
pub fn build() -> WorkflowDefinition {
    WorkflowBuilder::<serde_json::Value>::new()
        .start_with::<steps::FindOpenBaoPod>()
        .name("find-openbao-pod")
        .then::<steps::GetRootToken>()
        .name("get-root-token")
        .then::<steps::WriteSentinel>()
        .name("write-sentinel")
        .then::<steps::ApplyVaultAuth>()
        .name("apply-vault-auth")
        .then::<steps::ApplyVaultStaticSecret>()
        .name("apply-vault-static-secret")
        .then::<steps::WaitForSync>()
        .name("wait-for-sync")
        .then::<steps::CheckSecretValue>()
        .name("check-secret-value")
        .then::<steps::Cleanup>()
        .name("cleanup")
        .then::<steps::PrintResult>()
        .name("print-result")
        .end_workflow()
        .build("verify", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_returns_valid_definition() {
        let def = build();
        assert_eq!(def.id, "verify");
        assert_eq!(def.version, 1);
        assert_eq!(def.steps.len(), 9);
    }

    #[test]
    fn test_build_step_names() {
        let def = build();
        let names: Vec<Option<&str>> = def.steps.iter().map(|s| s.name.as_deref()).collect();
        assert_eq!(
            names,
            vec![
                Some("find-openbao-pod"),
                Some("get-root-token"),
                Some("write-sentinel"),
                Some("apply-vault-auth"),
                Some("apply-vault-static-secret"),
                Some("wait-for-sync"),
                Some("check-secret-value"),
                Some("cleanup"),
                Some("print-result"),
            ]
        );
    }
}
