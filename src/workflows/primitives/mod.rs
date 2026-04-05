//! Atomic workflow primitives.
//!
//! Each step does exactly one thing, reads its config from `ctx.step.step_config`,
//! and can be composed with `.config(json!({...}))` in workflow definitions.

mod apply_manifest;
mod collect_credentials;
pub mod kv_service_configs;
mod create_k8s_secret;
mod create_pg_database;
mod create_pg_role;
mod ensure_namespace;
mod opensearch_ml;
mod seed_kv_path;
mod vault_auth;
mod wait_for_rollout;
mod write_kv_path;

pub use apply_manifest::ApplyManifest;
pub use collect_credentials::CollectCredentials;
pub use create_k8s_secret::CreateK8sSecret;
pub use create_pg_database::CreatePGDatabase;
pub use create_pg_role::CreatePGRole;
pub use ensure_namespace::EnsureNamespace;
pub use opensearch_ml::{EnsureOpenSearchML, InjectOpenSearchModelId};
pub use seed_kv_path::SeedKVPath;
pub use vault_auth::{EnableVaultAuth, WriteVaultAuthConfig, WriteVaultPolicy, WriteVaultRole};
pub use wait_for_rollout::WaitForRollout;
pub use write_kv_path::WriteKVPath;
