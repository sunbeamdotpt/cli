//! Bootstrap workflow steps — Gitea admin setup, org creation, OIDC configuration.

mod bootstrap;

pub use bootstrap::{
    ConfigureOIDC, CreateOrgs, GetAdminPassword, MarkAdminPrivate, PrintBootstrapResult,
    SetAdminPassword, WaitForGiteaPod,
};
