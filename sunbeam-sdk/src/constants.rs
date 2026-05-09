//! Shared constants used across multiple modules.

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
