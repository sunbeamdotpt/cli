//! Seed workflow steps — each module contains one or more WFE step structs.

pub mod k8s_secrets;
/// Kratos admin.
pub mod kratos_admin;
/// Kv seeding.
pub mod kv_seeding;
/// Openbao init.
pub mod openbao_init;
/// Postgres.
pub mod postgres;

pub use k8s_secrets::SyncGiteaAdminPassword;
pub use kratos_admin::{PrintSeedOutputs, SeedKratosAdminIdentity};
pub use openbao_init::{FindOpenBaoPod, InitOrUnsealOpenBao, WaitPodRunning};
pub use postgres::{ConfigureDatabaseEngine, WaitForPostgres};
