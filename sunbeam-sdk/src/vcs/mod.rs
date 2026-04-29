//! `sunbeam vcs …` — gRPC client for gitserv.
//!
//! M-1 verb surface: `repo`, `mirror`, `ref`. Each leaf command marshals
//! clap args into a `gitserv-proto` request, invokes the authenticated
//! tonic client, and renders the response via the shared
//! [`crate::wfectl::output`] helpers (table default, `--format json`
//! opt-in).

pub mod admin;
pub mod client;
pub mod maintenance;
pub mod mirror;
pub mod ref_cmd;
pub mod repo;
pub mod signing_key;

use crate::error::{Result, SunbeamError};
use crate::output::OutputFormat;

#[derive(Debug, clap::Args)]
pub struct VcsArgs {
    /// Output format (default: table on TTY).
    #[arg(short = 'f', long, value_enum, default_value_t = OutputFormat::Table, global = true)]
    pub format: OutputFormat,

    #[command(subcommand)]
    pub command: VcsCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum VcsCommand {
    /// Repository lifecycle (create/get/list/delete/fork).
    Repo(repo::RepoArgs),
    /// Mirror management (pull/push mirrors + manual sync).
    Mirror(mirror::MirrorArgs),
    /// Ref inspection (list/get refs for a repo).
    Ref(ref_cmd::RefArgs),
    /// Admin surface: org creation, membership, and relation management.
    Admin(admin::AdminArgs),
    /// OpenPGP signing-key management (upload/list/revoke/bundle/rotate).
    #[command(name = "signing-key")]
    SigningKey(signing_key::SigningKeyArgs),
    /// Per-repo git maintenance (repack, bitmap regen, retention sweep).
    Maintenance(maintenance::MaintenanceArgs),
}

/// Dispatch a `sunbeam vcs …` invocation.
pub async fn handle(args: VcsArgs, domain: &str) -> Result<()> {
    if domain.is_empty() {
        return Err(SunbeamError::Config(
            "domain not set — run `sunbeam config set --domain <domain>` first".into(),
        ));
    }
    let endpoint = format!("https://source.{domain}:443");

    match args.command {
        VcsCommand::Repo(repo_args) => {
            tracing::debug!(?repo_args, "vcs repo");
            repo::run(repo_args, &endpoint, args.format).await
        }
        VcsCommand::Mirror(mirror_args) => {
            tracing::debug!(?mirror_args, "vcs mirror");
            mirror::run(mirror_args, &endpoint, args.format).await
        }
        VcsCommand::Ref(ref_args) => {
            tracing::debug!(?ref_args, "vcs ref");
            ref_cmd::run(ref_args, &endpoint, args.format).await
        }
        VcsCommand::Admin(admin_args) => {
            tracing::debug!(?admin_args, "vcs admin");
            admin::run(admin_args, &endpoint, args.format).await
        }
        VcsCommand::SigningKey(sk_args) => {
            tracing::debug!(?sk_args, "vcs signing-key");
            signing_key::run(sk_args, &endpoint, args.format).await
        }
        VcsCommand::Maintenance(maint_args) => {
            tracing::debug!(?maint_args, "vcs maintenance");
            maintenance::run(maint_args, &endpoint, args.format).await
        }
    }
}
