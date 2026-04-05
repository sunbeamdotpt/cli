//! Bootstrap workflow definition — Gitea admin setup sequence.

use wfe_core::builder::WorkflowBuilder;
use wfe_core::models::WorkflowDefinition;

use super::steps;

/// Build the bootstrap workflow definition.
///
/// Steps execute sequentially:
/// 1. Get admin password from K8s secret
/// 2. Wait for Gitea pod to be ready
/// 3. Set admin password
/// 4. Mark admin as private
/// 5. Create orgs (studio, internal)
/// 6. Configure OIDC auth source
/// 7. Print result
pub fn build() -> WorkflowDefinition {
    WorkflowBuilder::<serde_json::Value>::new()
        .start_with::<steps::GetAdminPassword>()
        .name("get-admin-password")
        .then::<steps::WaitForGiteaPod>()
        .name("wait-for-gitea-pod")
        .then::<steps::SetAdminPassword>()
        .name("set-admin-password")
        .then::<steps::MarkAdminPrivate>()
        .name("mark-admin-private")
        .then::<steps::CreateOrgs>()
        .name("create-orgs")
        .then::<steps::ConfigureOIDC>()
        .name("configure-oidc")
        .then::<steps::PrintBootstrapResult>()
        .name("print-bootstrap-result")
        .end_workflow()
        .build("bootstrap", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_returns_valid_definition() {
        let def = build();
        assert_eq!(def.id, "bootstrap");
        assert_eq!(def.version, 1);
        assert_eq!(def.steps.len(), 7);
    }

    #[test]
    fn test_build_step_names() {
        let def = build();
        let names: Vec<Option<&str>> = def
            .steps
            .iter()
            .map(|s| s.name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec![
                Some("get-admin-password"),
                Some("wait-for-gitea-pod"),
                Some("set-admin-password"),
                Some("mark-admin-private"),
                Some("create-orgs"),
                Some("configure-oidc"),
                Some("print-bootstrap-result"),
            ]
        );
    }
}
