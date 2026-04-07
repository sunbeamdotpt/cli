//! Shared constants used across multiple modules.

pub const GITEA_ADMIN_USER: &str = "gitea_admin";

/// Deprecated: prefer `registry::discover()` → `ServiceRegistry::namespaces()`.
pub const MANAGED_NS: &[&str] = &[
    "data",
    "devtools",
    "ingress",
    "matrix",
    "media",
    "monitoring",
    "ory",
    "stalwart",
    "storage",
    "vault-secrets-operator",
    "vpn",
    "wfe",
];
