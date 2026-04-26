//! `sunbeam vcs mirror …` — pull/push mirror config + manual sync.

use crate::error::Result;
use crate::output::{OutputFormat, render};
use crate::vcs::client::{connect_repo_client, map_status, resolve_token};
use gitserv_proto::pb::{
    AddPushMirrorRequest, CreatePullMirrorRequest, GetMirrorStatusRequest, RepoId,
    TriggerMirrorSyncRequest,
};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct MirrorArgs {
    #[command(subcommand)]
    pub command: MirrorCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum MirrorCmd {
    /// Create a pull mirror against an upstream URL.
    CreatePull {
        /// Repo ULID.
        repo_id: String,
        /// Upstream URL.
        upstream_url: String,
        /// Sync interval (seconds).
        #[arg(long, default_value_t = 3600)]
        interval_seconds: i64,
        /// Credential ref (e.g. `openbao://...`).
        #[arg(long)]
        credential_ref: Option<String>,
    },
    /// Register a push mirror writing this repo to a downstream URL.
    AddPush {
        /// Repo ULID.
        repo_id: String,
        /// Downstream target URL.
        target_url: String,
        /// Credential ref (e.g. `openbao://...`).
        #[arg(long)]
        credential_ref: Option<String>,
    },
    /// Trigger an immediate mirror sync.
    Sync {
        /// Mirror ULID.
        mirror_id: String,
    },
    /// Report mirror status + last-sync timestamp.
    Status {
        /// Mirror ULID.
        mirror_id: String,
    },
}

#[derive(Serialize)]
struct MirrorRow {
    ulid: String,
    repo_id: String,
    kind: String,
    target_url: String,
}

#[derive(Serialize)]
struct MirrorStatusRow {
    mirror_ulid: String,
    last_sync_ts: i64,
    last_error: String,
    credential_ok: bool,
}

pub async fn run(args: MirrorArgs, endpoint: &str, format: OutputFormat) -> Result<()> {
    let domain = crate::config::domain().to_string();
    let token = resolve_token(&domain)?;
    let mut client = connect_repo_client(endpoint, &token).await?;

    match args.command {
        MirrorCmd::CreatePull {
            repo_id,
            upstream_url,
            interval_seconds,
            credential_ref,
        } => {
            let cfg = client
                .create_pull_mirror(CreatePullMirrorRequest {
                    repo: Some(RepoId { ulid: repo_id }),
                    upstream_url,
                    credential_ref: credential_ref.unwrap_or_default(),
                    sync_interval_s: interval_seconds,
                    ref_allowlist: vec![],
                })
                .await
                .map_err(map_status)?
                .into_inner();
            render(&mirror_row(&cfg), format)
        }
        MirrorCmd::AddPush {
            repo_id,
            target_url,
            credential_ref,
        } => {
            let cfg = client
                .add_push_mirror(AddPushMirrorRequest {
                    repo: Some(RepoId { ulid: repo_id }),
                    downstream_url: target_url,
                    credential_ref: credential_ref.unwrap_or_default(),
                })
                .await
                .map_err(map_status)?
                .into_inner();
            render(&mirror_row(&cfg), format)
        }
        MirrorCmd::Sync { mirror_id } => {
            client
                .trigger_mirror_sync(TriggerMirrorSyncRequest {
                    mirror_ulid: mirror_id,
                })
                .await
                .map_err(map_status)?;
            Ok(())
        }
        MirrorCmd::Status { mirror_id } => {
            let resp = client
                .get_mirror_status(GetMirrorStatusRequest {
                    mirror_ulid: mirror_id,
                })
                .await
                .map_err(map_status)?
                .into_inner();
            render(
                &MirrorStatusRow {
                    mirror_ulid: resp.mirror_ulid,
                    last_sync_ts: resp.last_sync_ts,
                    last_error: resp.last_error,
                    credential_ok: resp.credential_ok,
                },
                format,
            )
        }
    }
}

fn mirror_row(cfg: &gitserv_proto::pb::MirrorConfig) -> MirrorRow {
    let repo_id = cfg.repo.as_ref().map(|r| r.ulid.clone()).unwrap_or_default();
    MirrorRow {
        ulid: cfg.ulid.clone(),
        repo_id,
        kind: cfg.kind.clone(),
        target_url: cfg.target_url.clone(),
    }
}
