//! Up workflow steps — each module contains one or more WFE step structs.

pub mod bootstrap_images;
pub mod build_images;
/// Certificate steps.
pub mod certificates;
/// Database.
pub mod database;
/// Finalize.
pub mod finalize;
/// Infrastructure.
pub mod infrastructure;
/// Lima VM lifecycle.
pub mod lima;
/// Platform.
pub mod platform;
/// Vault.
pub mod vault;
/// Vpn.
pub mod vpn;

// Steps unique to the up workflow
pub use bootstrap_images::BootstrapCriticalImages;
pub use build_images::BuildProjectImages;
pub use certificates::{EnsureTLSCert, EnsureTLSSecret, WaitForCertManagerWebhook};
pub use finalize::PrintURLs;
pub use infrastructure::{
    EnsureBuildKit, EnsureCilium, EnsureSeaweedFSBuckets, WaitForCNPGWebhook,
    WaitForLonghornWebhook,
};
pub use lima::EnsureLimaVm;
pub use vpn::MintVpnPreAuthKeys;

// Steps shared from the common steps pool (data-struct-agnostic, reusable)
pub use crate::workflows::steps::{
    ConfigureDatabaseEngine, FindOpenBaoPod, InitOrUnsealOpenBao, SeedKratosAdminIdentity,
    WaitForPostgres, WaitPodRunning,
};
