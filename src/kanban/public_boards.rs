//! Kanban public board commands (unauthenticated).

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;
use serde::Serialize;

use super::boards::visibility_name;
use super::required;
use crate::output::{OutputFormat, render, render_list};

/// Public board actions.
#[derive(Debug, Subcommand)]
pub enum PublicBoardAction {
    /// Get a public board.
    Get {
        /// Board ID or name.
        board_id: String,
    },
    /// List public boards in a project.
    List {
        /// Project ID or name.
        project_id: String,
    },
}

/// Serializable subset of a public board for CLI output.
#[derive(Serialize)]
struct BoardOut {
    id: String,
    project_id: String,
    name: String,
    description: String,
    icon: String,
    columns_count: i32,
    cards_count: i32,
    visibility: String,
}

impl From<v1::Board> for BoardOut {
    fn from(b: v1::Board) -> Self {
        Self {
            id: b.id,
            project_id: b.project_id,
            name: b.name,
            description: b.description,
            icon: b.icon,
            columns_count: b.columns_count,
            cards_count: b.cards_count,
            visibility: visibility_name(b.visibility.to_i32()),
        }
    }
}

/// Run a public board command.
pub(crate) async fn run(
    cmd: PublicBoardAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        PublicBoardAction::Get { board_id } => {
            let resp = client
                .public_boards()
                .get_public_board(v1::GetPublicBoardRequest {
                    board_id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            render(&BoardOut::from(required(resp.board, "board")?), format)
        }
        PublicBoardAction::List { project_id } => {
            let resp = client
                .public_boards()
                .list_public_boards(v1::ListPublicBoardsRequest {
                    project_id,
                    ..Default::default()
                })
                .await?
                .into_owned();
            let boards: Vec<_> = resp.boards.into_iter().map(BoardOut::from).collect();
            render_list(
                &boards,
                &["NAME", "COLUMNS", "CARDS", "VISIBILITY", "PROJECT", "ID"],
                |b| {
                    vec![
                        b.name.clone(),
                        b.columns_count.to_string(),
                        b.cards_count.to_string(),
                        b.visibility.clone(),
                        b.project_id.clone(),
                        b.id.clone(),
                    ]
                },
                format,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn board(id: &str, name: &str) -> v1::Board {
        v1::Board {
            id: id.to_string(),
            project_id: "proj_1".into(),
            name: name.to_string(),
            columns_count: 4,
            cards_count: 9,
            visibility: v1::BoardVisibility::Public.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn get_public_board() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.PublicBoardService/GetPublicBoard"))
            .respond_with(testutil::proto_response(&v1::GetPublicBoardResponse {
                board: board("board_1", "Roadmap").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            PublicBoardAction::Get {
                board_id: "board_1".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_public_boards_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(
                    "/sunbeam.kanban.v1.PublicBoardService/ListPublicBoards",
                ))
                .respond_with(testutil::proto_response(&v1::ListBoardsResponse {
                    boards: vec![board("board_1", "Roadmap")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                PublicBoardAction::List {
                    project_id: "proj_1".into(),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.PublicBoardService/GetPublicBoard"))
            .respond_with(testutil::connect_error(404, "not_found", "not public"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            PublicBoardAction::Get {
                board_id: "board_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not public"));
    }
}
