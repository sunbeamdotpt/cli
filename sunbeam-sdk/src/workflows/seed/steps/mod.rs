//! Seed workflow steps — each module contains one or more WFE step structs.

pub mod k8s_secrets;
pub mod kratos_admin;
pub mod kv_seeding;
pub mod openbao_init;
pub mod postgres;

pub use k8s_secrets::SyncGiteaAdminPassword;
pub use kratos_admin::{PrintSeedOutputs, SeedKratosAdminIdentity};
pub use openbao_init::{FindOpenBaoPod, InitOrUnsealOpenBao, WaitPodRunning};
pub use postgres::{ConfigureDatabaseEngine, WaitForPostgres};
