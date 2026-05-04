//! `sunbeam vcs ref …` — ref listing + lookup.
//!
//! Named `ref_cmd` to avoid collision with the `ref` keyword.

use buffa::MessageField;
use crate::error::Result;
use crate::output::{OutputFormat, render, render_list};
use crate::vcs::client::{connect_ref_client, map_status, resolve_token};
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
    let token = resolve_token(&domain).await?;
    let mut client = connect_ref_client(endpoint, &token).await?;

    match args.command {
        RefCmd::List { repo_id, prefix } => {
            let mut stream = client
                .list_refs(ListRefsRequest {
                    repo: MessageField::some(RepoId { ulid: repo_id, ..Default::default() }),
                    prefix: prefix.unwrap_or_default(),
                    ..Default::default()
                })
                .await
                .map_err(map_status)?;
            let mut rows: Vec<RefRow> = Vec::new();
            while let Some(entry) = stream.message().await.map_err(map_status)? {
                rows.push(RefRow {
                    name: entry.name.to_owned(),
                    oid: entry.oid.to_owned(),
                });
            }
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
                    repo: MessageField::some(RepoId { ulid: repo_id, ..Default::default() }),
                    name,
                    ..Default::default()
                })
                .await
                .map_err(map_status)?
                .into_owned();
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
