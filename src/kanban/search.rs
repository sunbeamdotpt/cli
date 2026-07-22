//! Kanban full-text card search.

use clap::Args;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;
use serde::Serialize;

use crate::output::{OutputFormat, render_list};

/// Search arguments.
#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Query string.
    pub query: String,
    /// Max results.
    #[arg(short, long)]
    pub limit: Option<i32>,
}

/// Serializable search hit.
#[derive(Serialize)]
struct SearchHitOut {
    card_id: String,
    card_ref: String,
    board_id: String,
    project_id: String,
    title: String,
    priority: String,
    status: String,
}

impl From<v1::CardSearchHit> for SearchHitOut {
    fn from(h: v1::CardSearchHit) -> Self {
        Self {
            card_id: h.card_id,
            card_ref: h.card_ref,
            board_id: h.board_id,
            project_id: h.project_id,
            title: h.title,
            priority: h.priority,
            status: h.status,
        }
    }
}

/// Run a search command.
pub(crate) async fn run(
    args: SearchArgs,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    let SearchArgs { query, limit } = args;

    let resp = client
        .search()
        .search_cards(v1::SearchCardsRequest {
            query,
            limit: limit.unwrap_or(20),
            ..Default::default()
        })
        .await?
        .into_owned();
    let hits: Vec<_> = resp.hits.into_iter().map(SearchHitOut::from).collect();

    render_list(
        &hits,
        &[
            "REF", "TITLE", "PRIORITY", "STATUS", "BOARD", "PROJECT", "CARD ID",
        ],
        |h| {
            vec![
                h.card_ref.clone(),
                h.title.clone(),
                h.priority.clone(),
                h.status.clone(),
                h.board_id.clone(),
                h.project_id.clone(),
                h.card_id.clone(),
            ]
        },
        format,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    #[tokio::test]
    async fn search_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.SearchService/SearchCards"))
                .respond_with(testutil::proto_response(&v1::SearchCardsResponse {
                    hits: vec![v1::CardSearchHit {
                        card_id: "card_1".into(),
                        card_ref: "BEAM-1".into(),
                        board_id: "board_1".into(),
                        project_id: "proj_1".into(),
                        title: "Fix crash".into(),
                        priority: "high".into(),
                        status: "open".into(),
                        ..Default::default()
                    }],
                    next_cursor: "cursor-2".into(),
                    total: 1,
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                SearchArgs {
                    query: "crash".into(),
                    limit: Some(10),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn search_defaults_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.SearchService/SearchCards"))
            .respond_with(testutil::proto_response(&v1::SearchCardsResponse::default()))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            SearchArgs {
                query: "crash".into(),
                limit: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.SearchService/SearchCards"))
            .respond_with(testutil::connect_error(401, "unauthenticated", "bad token"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            SearchArgs {
                query: "crash".into(),
                limit: None,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("bad token"));
    }
}
