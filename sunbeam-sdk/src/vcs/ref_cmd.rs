//! `sunbeam vcs ref …` — ref listing + lookup.
//!
//! Named `ref_cmd` to avoid collision with the `ref` keyword.

use crate::error::Result;
use crate::output::{OutputFormat, render, render_list};
use crate::vcs::client::{connect_ref_client, map_status, resolve_token};
use futures::StreamExt;
use gitserv_proto::pb::{GetRefRequest, ListRefsRequest, RepoId};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct RefArgs {
    #[command(subcommand)]
    pub command: RefCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum RefCmd {
    /// List refs in a repo, optionally filtered by prefix.
    List {
        /// Repo ULID.
        repo_id: String,
        /// Prefix filter (e.g. `refs/heads/`).
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Resolve a single ref to its OID.
    Get {
        /// Repo ULID.
        repo_id: String,
        /// Ref name (e.g. `refs/heads/main`).
        name: String,
    },
}

#[derive(Serialize)]
struct RefRow {
    name: String,
    oid: String,
}

pub async fn run(args: RefArgs, endpoint: &str, format: OutputFormat) -> Result<()> {
    let domain = crate::config::domain().to_string();
    let token = resolve_token(&domain)?;
    let mut client = connect_ref_client(endpoint, &token).await?;

    match args.command {
        RefCmd::List { repo_id, prefix } => {
            let stream = client
                .list_refs(ListRefsRequest {
                    repo: Some(RepoId { ulid: repo_id }),
                    prefix: prefix.unwrap_or_default(),
                })
                .await
                .map_err(map_status)?
                .into_inner();
            let rows: Vec<RefRow> = stream
                .filter_map(|r| async move {
                    r.ok().map(|e| RefRow {
                        name: e.name,
                        oid: e.oid,
                    })
                })
                .collect()
                .await;
            render_list(
                &rows,
                &["NAME", "OID"],
                |r| vec![r.name.clone(), r.oid.clone()],
                format,
            )
        }
        RefCmd::Get { repo_id, name } => {
            let resp = client
                .get_ref(GetRefRequest {
                    repo: Some(RepoId { ulid: repo_id }),
                    name,
                })
                .await
                .map_err(map_status)?
                .into_inner();
            render(
                &RefRow {
                    name: resp.name,
                    oid: resp.oid,
                },
                format,
            )
        }
    }
}
