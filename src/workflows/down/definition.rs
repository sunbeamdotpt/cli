//! Down workflow definition — cluster tear-down via WFE.
//!
//! Version 1: strict failures, sequenced teardown with force-cleanup.

use wfe_core::builder::WorkflowBuilder;
use wfe_core::models::{ErrorBehavior, WorkflowDefinition};

use super::steps;

/// Build the down workflow definition (version 1).
pub fn build() -> WorkflowDefinition {
    WorkflowBuilder::<serde_json::Value>::new()
        .default_error_behavior(ErrorBehavior::Terminate)
        .start_with::<steps::DiscoverNamespaces>()
        .name("discover-namespaces")
        .then::<steps::DeleteNamespaces>()
        .name("delete-namespaces")
        .then::<steps::WaitForTermination>()
        .name("wait-for-termination")
        .then::<steps::ForceDeleteStuckNamespaces>()
        .name("force-delete-stuck")
        .end_workflow()
        .build("down", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_returns_valid_definition() {
        let def = build();
        assert_eq!(def.id, "down");
        assert_eq!(def.version, 1);
        assert!(!def.steps.is_empty(), "expected non-empty steps");
    }

    #[test]
    fn test_default_error_behavior_is_terminate() {
        let def = build();
        assert_eq!(
            def.default_error_behavior,
            wfe_core::models::ErrorBehavior::Terminate,
            "down workflow should terminate immediately on any step failure"
        );
    }

    #[test]
    fn test_first_step_is_discover() {
        let def = build();
        assert_eq!(def.steps[0].name, Some("discover-namespaces".into()));
    }

    #[test]
    fn test_last_step_is_force_delete() {
        let def = build();
        let last = def.steps.last().unwrap();
        assert_eq!(last.name, Some("force-delete-stuck".into()));
    }
}
