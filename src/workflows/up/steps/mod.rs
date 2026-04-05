//! Up workflow steps — each module contains one or more WFE step structs.

pub mod certificates;
pub mod database;
pub mod finalize;
pub mod infrastructure;
pub mod platform;
pub mod vault;

// Steps unique to the up workflow
pub use certificates::{EnsureTLSCert, EnsureTLSSecret};
pub use finalize::PrintURLs;
pub use infrastructure::{EnsureBuildKit, EnsureCilium};
pub use platform::BootstrapGitea;

// Steps shared from seed workflow (data-struct-agnostic, reusable)
pub use crate::workflows::seed::steps::{
    FindOpenBaoPod, WaitPodRunning, InitOrUnsealOpenBao,
    WaitForPostgres,
    ConfigureDatabaseEngine,
    SyncGiteaAdminPassword,
};
