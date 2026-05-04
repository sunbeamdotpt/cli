//! `sunbeam vcs repo …` — repository CRUD + fork.

use buffa::MessageField;
use crate::error::Result;
use crate::output::{OutputFormat, render, render_list};
use crate::vcs::client::{connect_repo_client, map_status, resolve_token};
use gitserv_proto::pb::{
    CreateRepoRequest, DeleteRepoRequest, ForkRequest, GetRepoRequest, ListReposRequest, RepoId,
};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct RepoArgs {
    #[command(subcommand)]
    pub command: RepoCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum RepoCmd {
    /// Create a repository in an org.
    Create {
        /// Owning org slug.
        org: String,
        /// Repository slug.
        slug: String,
        /// Default branch (stored server-side; see plan F-list).
        #[arg(long)]
        default_branch: Option<String>,
    },
    /// Look up a repo by ULID (slug lookup lands in M-2).
    Get {
        /// Repo ULID.
        id: String,
    },
    /// List repos within an org.
    List {
        /// Org slug.
        org: String,
        /// Max rows per page.
        #[arg(long)]
        page_size: Option<i32>,
        /// Opaque server-provided page token.
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Delete a repo by ULID.
    Delete {
        /// Repo ULID.
        id: String,
    },
    /// Fork a repo into a target org/slug.
    Fork {
        /// Source repo ULID.
        source_id: String,
        /// Target org slug.
        target_org: String,
        /// Target repo slug.
        target_slug: String,
    },
}

#[derive(Serialize)]
struct RepoRow {
    id: String,
    full_name: String,
}

pub async fn run(args: RepoArgs, endpoint: &str, format: OutputFormat) -> Result<()> {
    let domain = crate::config::domain().to_string();
    let token = resolve_token(&domain).await?;
    let mut client = connect_repo_client(endpoint, &token).await?;

    match args.command {
        RepoCmd::Create {
            org,
            slug,
            default_branch: _default_branch,
        } => {
            let resp = client
                .create_repo(CreateRepoRequest {
                    org_slug: org.clone(),
                    repo_slug: slug.clone(),
                    ..Default::default()
                })
                .await
                .map_err(map_status)?
                .into_owned();
            let id = resp.id.into_option().map(|r| r.ulid).unwrap_or_default();
            render(
                &RepoRow {
                    id,
                    full_name: format!("{org}/{slug}"),
                },
                format,
            )
        }
        RepoCmd::Get { id } => {
            let resp = client
                .get_repo(GetRepoRequest {
                    id: MessageField::some(RepoId { ulid: id, ..Default::default() }),
                    ..Default::default()
                })
                .await
                .map_err(map_status)?
                .into_owned();
            render(&render_repo(&resp), format)
        }
        RepoCmd::List {
            org,
            page_size,
            page_token,
        } => {
            let resp = client
                .list_repos(ListReposRequest {
                    org_slug: org,
                    limit: page_size.unwrap_or(0),
                    page_token: page_token.unwrap_or_default(),
                    ..Default::default()
                })
                .await
                .map_err(map_status)?
                .into_owned();
            let rows: Vec<RepoRow> = resp.repos.iter().map(render_repo).collect();
            render_list(
                &rows,
                &["ID", "FULL_NAME"],
                |r| vec![r.id.clone(), r.full_name.clone()],
                format,
            )
        }
        RepoCmd::Delete { id } => {
            client
                .delete_repo(DeleteRepoRequest {
                    id: MessageField::some(RepoId { ulid: id, ..Default::default() }),
                    ..Default::default()
                })
                .await
                .map_err(map_status)?;
            Ok(())
        }
        RepoCmd::Fork {
            source_id,
            target_org,
            target_slug,
        } => {
            let resp = client
                .fork(ForkRequest {
                    source: MessageField::some(RepoId { ulid: source_id, ..Default::default() }),
                    target_org_slug: target_org.clone(),
                    target_repo_slug: target_slug.clone(),
                    ..Default::default()
                })
                .await
                .map_err(map_status)?
                .into_owned();
            let id = resp.id.into_option().map(|r| r.ulid).unwrap_or_default();
            render(
                &RepoRow {
                    id,
                    full_name: format!("{target_org}/{target_slug}"),
                },
                format,
            )
        }
    }
}

fn render_repo(r: &gitserv_proto::pb::GetRepoResponse) -> RepoRow {
    let id = r.id.as_option().map(|i| i.ulid.clone()).unwrap_or_default();
    RepoRow {
        id,
        full_name: format!("{}/{}", r.org_slug, r.repo_slug),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_repo_builds_full_name_from_proto() {
        let proto = gitserv_proto::pb::GetRepoResponse {
            id: buffa::MessageField::some(gitserv_proto::pb::RepoId {
                ulid: "01K000000000000000000000A".into(),
                ..Default::default()
            }),
            org_slug: "acme".into(),
            repo_slug: "widgets".into(),
            ..Default::default()
        };
        let row = render_repo(&proto);
        assert_eq!(row.full_name, "acme/widgets");
        assert_eq!(row.id, "01K000000000000000000000A");
    }

    #[test]
    fn repo_row_serializes_to_json_with_full_name() {
        let row = RepoRow {
            id: "abc".into(),
            full_name: "acme/widgets".into(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"full_name\":\"acme/widgets\""), "{json}");
    }
}
