//! Kanban GitHub link commands.

use clap::Subcommand;
use sdk::error::{Result, ResultExt, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, mutating_options, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// GitHub link actions.
#[derive(Debug, Clone, Subcommand)]
pub enum GitHubAction {
    /// Link a GitHub issue/PR to a card.
    Link {
        /// Card ID.
        card_id: String,
        /// Issue reference: owner/repo#number.
        issue: String,
    },
    /// Unlink a GitHub issue/PR.
    Unlink {
        /// Card ID.
        card: String,
        /// Link ID.
        link_id: String,
    },
    /// List links for a card.
    List {
        /// Card ID.
        card_id: String,
    },
    /// Search GitHub issues.
    Search {
        /// Card ID.
        card: String,
        /// Repository: owner/repo.
        repo: String,
        /// Search query.
        query: String,
    },
    /// Resync a link.
    Resync {
        /// Card ID.
        card: String,
        /// Link ID.
        link_id: String,
    },
}

/// Serializable GitHub link detail record.
#[derive(Serialize)]
struct GitHubLinkDetailOut {
    id: String,
    card_id: String,
    repo_owner: String,
    repo_name: String,
    issue_or_pr_number: i32,
    kind: String,
    title: String,
    state: String,
    url: String,
    last_synced_at: String,
}

impl From<v1::GitHubLinkDetail> for GitHubLinkDetailOut {
    fn from(l: v1::GitHubLinkDetail) -> Self {
        Self {
            id: l.id,
            card_id: l.card_id,
            repo_owner: l.repo_owner,
            repo_name: l.repo_name,
            issue_or_pr_number: l.issue_or_pr_number,
            kind: l.kind,
            title: l.title,
            state: l.state,
            url: l.url,
            last_synced_at: fmt_ts(&l.last_synced_at),
        }
    }
}

/// Serializable GitHub issue search result.
#[derive(Serialize)]
struct GitHubIssueResultOut {
    repo_owner: String,
    repo_name: String,
    number: i32,
    kind: String,
    title: String,
    state: String,
    url: String,
}

impl From<v1::GitHubIssueResult> for GitHubIssueResultOut {
    fn from(i: v1::GitHubIssueResult) -> Self {
        Self {
            repo_owner: i.repo_owner,
            repo_name: i.repo_name,
            number: i.number,
            kind: i.kind,
            title: i.title,
            state: i.state,
            url: i.url,
        }
    }
}

/// Parse `owner/repo#number` into its components.
fn parse_issue_ref(issue: &str) -> Result<(String, String, i32)> {
    let (repo, num_str) = issue
        .rsplit_once('#')
        .with_ctx(|| format!("invalid issue reference {issue}: expected owner/repo#number"))?;
    let number = num_str
        .parse::<i32>()
        .map_err(|e| SunbeamError::Other(format!("invalid issue number {num_str}: {e}")))?;
    let (owner, name) = repo
        .split_once('/')
        .with_ctx(|| format!("invalid repository {repo}: expected owner/repo"))?;
    Ok((owner.to_string(), name.to_string(), number))
}

/// Parse `owner/repo` into its components.
fn parse_repo(repo: &str) -> Result<(String, String)> {
    let (owner, name) = repo
        .split_once('/')
        .with_ctx(|| format!("invalid repository {repo}: expected owner/repo"))?;
    Ok((owner.to_string(), name.to_string()))
}

/// Run a GitHub link command.
pub(crate) async fn run(
    cmd: GitHubAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        GitHubAction::Link { card_id, issue } => {
            let (repo_owner, repo_name, number) = parse_issue_ref(&issue)?;
            let resp = client
                .github_links()
                .link_issue_with_options(
                    v1::LinkIssueRequest {
                        card_id: card_id.clone(),
                        repo_owner,
                        repo_name,
                        number,
                        ..Default::default()
                    },
                    mutating_options(&card_id),
                )
                .await?
                .into_owned();
            render(
                &GitHubLinkDetailOut::from(required(resp.link, "link")?),
                format,
            )
        }
        GitHubAction::Unlink { card, link_id } => {
            client
                .github_links()
                .unlink_issue_with_options(
                    v1::UnlinkIssueRequest {
                        link_id: link_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&card),
                )
                .await?;
            render(
                &serde_json::json!({
                    "unlinked": true,
                    "link_id": link_id,
                }),
                format,
            )
        }
        GitHubAction::List { card_id } => {
            let resp = client
                .github_links()
                .list_links_by_card_with_options(
                    v1::ListLinksByCardRequest {
                        card_id: card_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&card_id),
                )
                .await?
                .into_owned();
            let links: Vec<_> = resp
                .links
                .into_iter()
                .map(GitHubLinkDetailOut::from)
                .collect();
            render_list(
                &links,
                &["REPO", "NUMBER", "KIND", "TITLE", "STATE", "URL", "ID"],
                |l| {
                    vec![
                        format!("{}/{}", l.repo_owner, l.repo_name),
                        l.issue_or_pr_number.to_string(),
                        l.kind.clone(),
                        l.title.clone(),
                        l.state.clone(),
                        l.url.clone(),
                        l.id.clone(),
                    ]
                },
                format,
            )
        }
        GitHubAction::Search { card, repo, query } => {
            let (repo_owner, repo_name) = parse_repo(&repo)?;
            let resp = client
                .github_links()
                .search_github_issues_with_options(
                    v1::SearchGithubIssuesRequest {
                        repo_owner,
                        repo_name,
                        query,
                        limit: 20,
                        ..Default::default()
                    },
                    object_id_options(&card),
                )
                .await?
                .into_owned();
            let results: Vec<_> = resp
                .results
                .into_iter()
                .map(GitHubIssueResultOut::from)
                .collect();
            render_list(
                &results,
                &["REPO", "NUMBER", "KIND", "TITLE", "STATE", "URL"],
                |r| {
                    vec![
                        format!("{}/{}", r.repo_owner, r.repo_name),
                        r.number.to_string(),
                        r.kind.clone(),
                        r.title.clone(),
                        r.state.clone(),
                        r.url.clone(),
                    ]
                },
                format,
            )
        }
        GitHubAction::Resync { card, link_id } => {
            let resp = client
                .github_links()
                .resync_link_with_options(
                    v1::ResyncLinkRequest {
                        link_id: link_id.clone(),
                        ..Default::default()
                    },
                    mutating_options(&card),
                )
                .await?
                .into_owned();
            render(
                &GitHubLinkDetailOut::from(required(resp.link, "link")?),
                format,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use sdk::kanban::prelude::buffa_types;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn link_detail(id: &str) -> v1::GitHubLinkDetail {
        v1::GitHubLinkDetail {
            id: id.to_string(),
            card_id: "card_1".into(),
            repo_owner: "sunbeam".into(),
            repo_name: "cli".into(),
            issue_or_pr_number: 42,
            kind: "issue".into(),
            title: "Bug".into(),
            state: "open".into(),
            url: "https://github.com/sunbeam/cli/issues/42".into(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_issue_ref_splits_components() {
        let (owner, name, number) = parse_issue_ref("sunbeam/cli#42").unwrap();
        assert_eq!(
            (owner.as_str(), name.as_str(), number),
            ("sunbeam", "cli", 42)
        );
        assert!(parse_issue_ref("sunbeam/cli").is_err());
        assert!(parse_issue_ref("sunbeam/cli#abc").is_err());
        assert!(parse_issue_ref("cli#42").is_err());
    }

    #[test]
    fn parse_repo_splits_owner_name() {
        let (owner, name) = parse_repo("sunbeam/cli").unwrap();
        assert_eq!((owner.as_str(), name.as_str()), ("sunbeam", "cli"));
        assert!(parse_repo("cli").is_err());
    }

    #[tokio::test]
    async fn link_and_resync() {
        let server = MockServer::start().await;
        for (rpc, body) in [
            (
                "LinkIssue",
                testutil::proto_response(&v1::LinkIssueResponse {
                    link: link_detail("gh_1").into(),
                    ..Default::default()
                }),
            ),
            (
                "ResyncLink",
                testutil::proto_response(&v1::ResyncLinkResponse {
                    link: link_detail("gh_1").into(),
                    ..Default::default()
                }),
            ),
        ] {
            Mock::given(method("POST"))
                .and(path(format!("/sunbeam.kanban.v1.GithubLinkService/{rpc}")))
                .respond_with(body)
                .mount(&server)
                .await;
        }

        let client = testutil::client_for(&server.uri());
        run(
            GitHubAction::Link {
                card_id: "card_1".into(),
                issue: "sunbeam/cli#42".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            GitHubAction::Resync {
                card: "card_1".into(),
                link_id: "gh_1".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unlink() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.GithubLinkService/UnlinkIssue"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            GitHubAction::Unlink {
                card: "card_1".into(),
                link_id: "gh_1".into(),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_links_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.GithubLinkService/ListLinksByCard"))
                .respond_with(testutil::proto_response(&v1::ListLinksByCardResponse {
                    links: vec![link_detail("gh_1")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                GitHubAction::List {
                    card_id: "card_1".into(),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn search_issues() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.GithubLinkService/SearchGithubIssues",
            ))
            .respond_with(testutil::proto_response(&v1::SearchGithubIssuesResponse {
                results: vec![v1::GitHubIssueResult {
                    repo_owner: "sunbeam".into(),
                    repo_name: "cli".into(),
                    number: 42,
                    kind: "issue".into(),
                    title: "Bug".into(),
                    state: "open".into(),
                    url: "https://github.com/sunbeam/cli/issues/42".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            GitHubAction::Search {
                card: "card_1".into(),
                repo: "sunbeam/cli".into(),
                query: "bug".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn link_rejects_bad_issue_ref() {
        let client = testutil::client_for("http://127.0.0.1:1");
        let err = run(
            GitHubAction::Link {
                card_id: "card_1".into(),
                issue: "not-a-ref".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid issue reference"));
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.GithubLinkService/ListLinksByCard"))
            .respond_with(testutil::connect_error(502, "unavailable", "github down"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            GitHubAction::List {
                card_id: "card_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("github down"));
    }
}
