//! Platform steps — currently empty after Gitea removal.
//!
//! Gitea was removed from the core stack in v3. The old BootstrapGitea step
//! (which created orgs, set admin password, and configured OIDC) is gone.
//! If Gitea is re-added in the future, a generic ConfigureOIDC step should
//! live here.

#[cfg(test)]
mod tests {
    #[test]
    fn platform_steps_placeholder() {
        // Placeholder so the module compiles even when empty.
    }
}
