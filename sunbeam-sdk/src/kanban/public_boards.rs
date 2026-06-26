//! Kanban public board commands (unauthenticated).

use crate::error::Result;
use crate::kanban::client::{self, PublicBoardServiceClient};
use crate::logger::Logger;
use crate::output::{render, render_list, OutputFormat};
use async_trait::async_trait;
use clap::Subcommand;
use serde::Serialize;

/// Public board actions.
#[derive(Debug, Subcommand)]
pub enum PublicBoardAction {
    /// Get a public board.
    Get {
        /// Board ID.
        board_id: String,
    },
    /// List public boards in a project.
    List {
        /// Project ID.
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

fn visibility_str(value: i32) -> String {
    match value {
        1 => "private".to_string(),
        2 => "internal".to_string(),
        3 => "public".to_string(),
        _ => "unspecified".to_string(),
    }
}

fn board_to_out(board: &client::Board) -> BoardOut {
    BoardOut {
        id: board.id.clone(),
        project_id: board.project_id.clone(),
        name: board.name.clone(),
        description: board.description.clone(),
        icon: board.icon.clone(),
        columns_count: board.columns_count,
        cards_count: board.cards_count,
        visibility: visibility_str(board.visibility),
    }
}

/// Trait abstracting the Kanban public board service for testability.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait PublicBoardService {
    /// Get a public board by ID.
    async fn get_public_board(
        &mut self,
        req: client::GetPublicBoardRequest,
    ) -> Result<client::Board>;
    /// List public boards in a project.
    async fn list_public_boards(
        &mut self,
        req: client::ListPublicBoardsRequest,
    ) -> Result<client::ListBoardsResponse>;
}

/// Wrapper around the generated Tonic public board client.
#[derive(Debug)]
pub struct PublicBoardServiceClientWrapper {
    inner: PublicBoardServiceClient<tonic::transport::Channel>,
}

impl PublicBoardServiceClientWrapper {
    /// Build a wrapper from an unauthenticated channel.
    pub fn new(inner: PublicBoardServiceClient<tonic::transport::Channel>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl PublicBoardService for PublicBoardServiceClientWrapper {
    async fn get_public_board(
        &mut self,
        req: client::GetPublicBoardRequest,
    ) -> Result<client::Board> {
        let resp = self.inner.get_public_board(req).await?;
        Ok(resp.into_inner())
    }

    async fn list_public_boards(
        &mut self,
        req: client::ListPublicBoardsRequest,
    ) -> Result<client::ListBoardsResponse> {
        let resp = self.inner.list_public_boards(req).await?;
        Ok(resp.into_inner())
    }
}

/// Build a public-board-service client wrapper for the given server.
pub async fn build_client(
    logger: &Logger,
    server: &str,
) -> Result<PublicBoardServiceClientWrapper> {
    let channel = client::connect(logger, server).await?;
    Ok(PublicBoardServiceClientWrapper::new(
        PublicBoardServiceClient::new(channel),
    ))
}

/// Run a public board command using the provided service client.
pub async fn run_with_client(
    cmd: PublicBoardAction,
    format: OutputFormat,
    client: &mut dyn PublicBoardService,
) -> Result<()> {
    match cmd {
        PublicBoardAction::Get { board_id } => {
            let req = client::GetPublicBoardRequest { board_id };
            let resp = client.get_public_board(req).await?;
            render(&board_to_out(&resp), format)
        }
        PublicBoardAction::List { project_id } => {
            let req = client::ListPublicBoardsRequest { project_id };
            let resp = client.list_public_boards(req).await?;
            let boards: Vec<_> = resp.boards.iter().map(board_to_out).collect();
            render_list(
                &boards,
                &["ID", "PROJECT", "NAME", "COLUMNS", "CARDS", "VISIBILITY"],
                |b| {
                    vec![
                        b.id.clone(),
                        b.project_id.clone(),
                        b.name.clone(),
                        b.columns_count.to_string(),
                        b.cards_count.to_string(),
                        b.visibility.clone(),
                    ]
                },
                format,
            )
        }
    }
}

/// Run a public board command.
pub async fn run(
    logger: &Logger,
    cmd: PublicBoardAction,
    server: &str,
    format: OutputFormat,
) -> Result<()> {
    let mut client = build_client(logger, server).await?;
    run_with_client(cmd, format, &mut client).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_board(id: &str, project_id: &str, visibility: i32) -> client::Board {
        client::Board {
            id: id.into(),
            project_id: project_id.into(),
            name: "Board".into(),
            description: "A board".into(),
            icon: "square".into(),
            created_at: Some(prost_types::Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            updated_at: Some(prost_types::Timestamp {
                seconds: 2,
                nanos: 0,
            }),
            columns_count: 3,
            cards_count: 5,
            visibility,
        }
    }

    #[tokio::test]
    async fn get_public_board_renders() {
        let mut mock = MockPublicBoardService::new();
        mock.expect_get_public_board()
            .withf(|req| req.board_id == "board_123")
            .times(1)
            .returning(|_| Ok(sample_board("board_123", "proj_123", 3)));

        run_with_client(
            PublicBoardAction::Get {
                board_id: "board_123".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_public_boards_renders() {
        let mut mock = MockPublicBoardService::new();
        mock.expect_list_public_boards()
            .withf(|req| req.project_id == "proj_123")
            .times(1)
            .returning(|_| {
                Ok(client::ListBoardsResponse {
                    boards: vec![sample_board("board_1", "proj_123", 3)],
                })
            });

        run_with_client(
            PublicBoardAction::List {
                project_id: "proj_123".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_public_boards_table_renders() {
        let mut mock = MockPublicBoardService::new();
        mock.expect_list_public_boards()
            .times(1)
            .returning(|_| {
                Ok(client::ListBoardsResponse {
                    boards: vec![
                        sample_board("b1", "p1", 1),
                        sample_board("b2", "p1", 2),
                        sample_board("b3", "p1", 99),
                    ],
                })
            });

        run_with_client(
            PublicBoardAction::List {
                project_id: "p1".into(),
            },
            OutputFormat::Table,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[test]
    fn visibility_strings_cover_all_branches() {
        assert_eq!(visibility_str(1), "private");
        assert_eq!(visibility_str(2), "internal");
        assert_eq!(visibility_str(3), "public");
        assert_eq!(visibility_str(0), "unspecified");
        assert_eq!(visibility_str(99), "unspecified");
    }

    #[tokio::test]
    async fn get_public_board_table_renders() {
        let mut mock = MockPublicBoardService::new();
        mock.expect_get_public_board()
            .times(1)
            .returning(|_| Ok(sample_board("board_123", "proj_123", 3)));

        run_with_client(
            PublicBoardAction::Get {
                board_id: "board_123".into(),
            },
            OutputFormat::Table,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn run_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = run(
            &logger,
            PublicBoardAction::Get {
                board_id: "board_123".into(),
            },
            ":::not-a-url",
            OutputFormat::Json,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }

    #[tokio::test]
    async fn build_client_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build_client(&logger, ":::not-a-url")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }

    #[tokio::test]
    async fn wrapper_new_constructs() {
        let channel = tonic::transport::Endpoint::from_static("http://[::1]:1")
            .connect_lazy();
        let _wrapper =
            PublicBoardServiceClientWrapper::new(PublicBoardServiceClient::new(channel));
    }
}
