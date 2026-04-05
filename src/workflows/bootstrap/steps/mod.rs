//! Bootstrap workflow steps — Gitea admin setup, org creation, OIDC configuration.

mod bootstrap;

pub use bootstrap::{
    GetAdminPassword,
    WaitForGiteaPod,
    SetAdminPassword,
    MarkAdminPrivate,
    CreateOrgs,
    ConfigureOIDC,
    PrintBootstrapResult,
};
