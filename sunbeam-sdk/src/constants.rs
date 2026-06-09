//! Shared constants used across multiple modules.

/// Lima VM name for the Sunbeam local dev stack.
pub const LIMA_VM_NAME: &str = "lima-sunbeam";

/// kubectl context name used for the Sunbeam Lima VM.
pub const LIMA_KUBE_CONTEXT: &str = "lima-sunbeam";

/// Deprecated: prefer `registry::discover()` → `ServiceRegistry::namespaces()`.
pub const MANAGED_NS: &[&str] = &[
    "data",
    "devtools",
    "ingress",
    "matrix",
    "media",
    "monitoring",
    "openbao",
    "ory",
    "stalwart",
    "storage",
    "vault-secrets-operator",
    "vpn",
    "wfe",
];
