//! Kanban board commands.

use crate::error::{Result, ResultExt};
use crate::kanban::client::{self, BoardServiceClient, BoardVisibility};
use crate::kanban::client::{
    AddColumnRequest, CreateBoardRequest, DeleteBoardRequest, GetBoardRequest, ListBoardsRequest,
    MoveColumnRequest, RemoveColumnRequest, UpdateBoardRequest, UpdateColumnRequest,
};
use crate::kanban::resolve;
use crate::logger::Logger;
use crate::output::{OutputFormat, render, render_list};
use crate::wfectl::output::fmt_proto_time;
use async_trait::async_trait;
use clap::Subcommand;
use prost_types::FieldMask;
use serde::Serialize;

/// Board actions.
#[derive(Debug, Subcommand)]
pub enum BoardAction {
    /// List boards.
    List {
        /// Project ID.
        #[arg(short, long)]
        project: String,
    },
    /// Get a board.
    Get {
        /// Board ID or name.
        board_id: String,
    },
    /// Create a board.
    Create {
        /// Project ID.
        #[arg(short, long)]
        project: String,
        /// Board name.
        #[arg(short, long)]
        name: String,
        /// Description.
        #[arg(short, long)]
        description: Option<String>,
        /// Icon identifier.
        #[arg(short, long)]
        icon: Option<String>,
        /// Visibility.
        #[arg(short, long, value_enum, default_value = "private")]
        visibility: VisibilityArg,
    },
    /// Update a board.
    Update {
        /// Board ID or name.
        board_id: String,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
        /// New description.
        #[arg(short, long)]
        description: Option<String>,
        /// New icon.
        #[arg(short, long)]
        icon: Option<String>,
        /// New visibility.
        #[arg(short, long, value_enum)]
        visibility: Option<VisibilityArg>,
    },
    /// Delete a board.
    Delete {
        /// Board ID or name.
        board_id: String,
    },
    /// Column management.
    Column {
        /// Column subcommand to run.
        #[command(subcommand)]
        action: ColumnAction,
    },
}

/// Visibility values matching `BoardVisibility`.
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum VisibilityArg {
    /// Private board.
    #[default]
    Private,
    /// Internal board.
    Internal,
    /// Public board.
    Public,
}

impl From<VisibilityArg> for i32 {
    fn from(v: VisibilityArg) -> Self {
        match v {
            VisibilityArg::Private => BoardVisibility::Private as i32,
            VisibilityArg::Internal => BoardVisibility::Internal as i32,
            VisibilityArg::Public => BoardVisibility::Public as i32,
        }
    }
}

/// Board column actions.
#[derive(Debug, Subcommand)]
pub enum ColumnAction {
    /// Add a column.
    Add {
        /// Board ID or name.
        board_id: String,
        /// Column title.
        #[arg(short, long)]
        title: String,
        /// Accent color.
        #[arg(short, long)]
        accent: Option<String>,
        /// WIP limit.
        #[arg(short, long)]
        wip_limit: Option<i32>,
        /// Position.
        #[arg(short, long)]
        position: Option<i32>,
    },
    /// Update a column.
    Update {
        /// Board ID or name.
        board_id: String,
        /// Column ID or title.
        column_id: String,
        /// New title.
        #[arg(short, long)]
        title: Option<String>,
        /// New accent.
        #[arg(short, long)]
        accent: Option<String>,
        /// New WIP limit.
        #[arg(short, long)]
        wip_limit: Option<i32>,
    },
    /// Remove a column.
    Remove {
        /// Board ID or name.
        board_id: String,
        /// Column ID or title.
        column_id: String,
    },
    /// Move a column.
    Move {
        /// Board ID or name.
        board_id: String,
        /// Column ID or title.
        column_id: String,
        /// New position.
        #[arg(short, long)]
        position: i32,
    },
}

/// Serializable board for output.
#[derive(Serialize)]
struct BoardOut {
    id: String,
    project_id: String,
    name: String,
    description: String,
    icon: String,
    created_at: String,
    updated_at: String,
    columns_count: i32,
    cards_count: i32,
    visibility: String,
}

impl From<client::Board> for BoardOut {
    fn from(b: client::Board) -> Self {
        Self {
            id: b.id,
            project_id: b.project_id,
            name: b.name,
            description: b.description,
            icon: b.icon,
            created_at: b
                .created_at
                .as_ref()
                .map(fmt_proto_time)
                .unwrap_or_default(),
            updated_at: b
                .updated_at
                .as_ref()
                .map(fmt_proto_time)
                .unwrap_or_default(),
            columns_count: b.columns_count,
            cards_count: b.cards_count,
            visibility: board_visibility_name(b.visibility),
        }
    }
}

/// Serializable column for output.
#[derive(Serialize)]
struct ColumnOut {
    id: String,
    board_id: String,
    title: String,
    accent: String,
    wip_limit: i32,
    position: i32,
    created_at: String,
    updated_at: String,
}

impl From<client::Column> for ColumnOut {
    fn from(c: client::Column) -> Self {
        Self {
            id: c.id,
            board_id: c.board_id,
            title: c.title,
            accent: c.accent,
            wip_limit: c.wip_limit,
            position: c.position,
            created_at: c
                .created_at
                .as_ref()
                .map(fmt_proto_time)
                .unwrap_or_default(),
            updated_at: c
                .updated_at
                .as_ref()
                .map(fmt_proto_time)
                .unwrap_or_default(),
        }
    }
}

/// Serializable board detail for output.
#[derive(Serialize)]
struct BoardDetailOut {
    #[serde(flatten)]
    board: BoardOut,
    columns: Vec<ColumnOut>,
}

fn board_visibility_name(v: i32) -> String {
    match v {
        v if v == BoardVisibility::Private as i32 => "private".to_string(),
        v if v == BoardVisibility::Internal as i32 => "internal".to_string(),
        v if v == BoardVisibility::Public as i32 => "public".to_string(),
        _ => "unspecified".to_string(),
    }
}

/// Trait abstracting the Kanban board service for testability.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait BoardService {
    /// List boards in a project.
    async fn list_boards(
        &mut self,
        req: tonic::Request<client::ListBoardsRequest>,
    ) -> Result<client::ListBoardsResponse>;

    /// Get a board with its columns.
    async fn get_board(
        &mut self,
        req: tonic::Request<client::GetBoardRequest>,
    ) -> Result<client::BoardDetail>;

    /// Create a board.
    async fn create_board(
        &mut self,
        req: tonic::Request<client::CreateBoardRequest>,
    ) -> Result<client::Board>;

    /// Update a board.
    async fn update_board(
        &mut self,
        req: tonic::Request<client::UpdateBoardRequest>,
    ) -> Result<client::Board>;

    /// Delete a board.
    async fn delete_board(&mut self, req: tonic::Request<client::DeleteBoardRequest>)
    -> Result<()>;

    /// Add a column to a board.
    async fn add_column(
        &mut self,
        req: tonic::Request<client::AddColumnRequest>,
    ) -> Result<client::Column>;

    /// Update a column.
    async fn update_column(
        &mut self,
        req: tonic::Request<client::UpdateColumnRequest>,
    ) -> Result<client::Column>;

    /// Remove a column from a board.
    async fn remove_column(
        &mut self,
        req: tonic::Request<client::RemoveColumnRequest>,
    ) -> Result<()>;

    /// Move a column within a board.
    async fn move_column(
        &mut self,
        req: tonic::Request<client::MoveColumnRequest>,
    ) -> Result<client::MoveColumnResponse>;
}

/// Wrapper around the generated Tonic board client.
#[derive(Debug)]
pub struct BoardServiceClientWrapper {
    inner: BoardServiceClient<client::AuthChannel>,
}

impl BoardServiceClientWrapper {
    /// Build a wrapper from an authenticated channel.
    pub fn new(inner: BoardServiceClient<client::AuthChannel>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl BoardService for BoardServiceClientWrapper {
    async fn list_boards(
        &mut self,
        req: tonic::Request<client::ListBoardsRequest>,
    ) -> Result<client::ListBoardsResponse> {
        let resp = self.inner.list_boards(req).await?;
        Ok(resp.into_inner())
    }

    async fn get_board(
        &mut self,
        req: tonic::Request<client::GetBoardRequest>,
    ) -> Result<client::BoardDetail> {
        let resp = self.inner.get_board(req).await?;
        Ok(resp.into_inner())
    }

    async fn create_board(
        &mut self,
        req: tonic::Request<client::CreateBoardRequest>,
    ) -> Result<client::Board> {
        let resp = self.inner.create_board(req).await?;
        Ok(resp.into_inner())
    }

    async fn update_board(
        &mut self,
        req: tonic::Request<client::UpdateBoardRequest>,
    ) -> Result<client::Board> {
        let resp = self.inner.update_board(req).await?;
        Ok(resp.into_inner())
    }

    async fn delete_board(
        &mut self,
        req: tonic::Request<client::DeleteBoardRequest>,
    ) -> Result<()> {
        self.inner.delete_board(req).await?;
        Ok(())
    }

    async fn add_column(
        &mut self,
        req: tonic::Request<client::AddColumnRequest>,
    ) -> Result<client::Column> {
        let resp = self.inner.add_column(req).await?;
        Ok(resp.into_inner())
    }

    async fn update_column(
        &mut self,
        req: tonic::Request<client::UpdateColumnRequest>,
    ) -> Result<client::Column> {
        let resp = self.inner.update_column(req).await?;
        Ok(resp.into_inner())
    }

    async fn remove_column(
        &mut self,
        req: tonic::Request<client::RemoveColumnRequest>,
    ) -> Result<()> {
        self.inner.remove_column(req).await?;
        Ok(())
    }

    async fn move_column(
        &mut self,
        req: tonic::Request<client::MoveColumnRequest>,
    ) -> Result<client::MoveColumnResponse> {
        let resp = self.inner.move_column(req).await?;
        Ok(resp.into_inner())
    }
}

/// Build a board-service client wrapper for the given server and token.
pub async fn build_client(
    logger: &Logger,
    server: &str,
    token: &str,
) -> Result<BoardServiceClientWrapper> {
    let channel = client::build(logger, server, token).await?;
    Ok(BoardServiceClientWrapper::new(BoardServiceClient::new(
        channel,
    )))
}

/// Resolve a column identifier from a raw string within a board.
///
/// If `raw` is ID-shaped it is returned unchanged; otherwise the board detail
/// is fetched and the unique column title match is returned.
async fn resolve_column_id(
    client: &mut dyn BoardService,
    board_id: &str,
    raw: &str,
) -> Result<String> {
    if resolve::looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let resp = client
        .get_board(tonic::Request::new(GetBoardRequest {
            board_id: board_id.to_string(),
        }))
        .await
        .with_ctx(|| format!("get board {board_id} to resolve column name"))?;
    let matches: Vec<_> = resp
        .columns
        .into_iter()
        .filter(|c| resolve::name_matches(&c.title, raw))
        .map(|c| (c.id, c.title))
        .collect();
    resolve::unique_match(matches, "column", raw)
}

/// Run a board command.
pub async fn run(
    cmd: BoardAction,
    format: OutputFormat,
    client: &mut dyn BoardService,
) -> Result<()> {
    match cmd {
        BoardAction::List { project } => {
            let resp = client
                .list_boards(tonic::Request::new(ListBoardsRequest {
                    project_id: project.clone(),
                }))
                .await
                .with_ctx(|| "list boards failed".to_string())?;
            let boards: Vec<BoardOut> = resp.boards.into_iter().map(Into::into).collect();
            render_list(
                &boards,
                &["ID", "PROJECT ID", "NAME", "VISIBILITY", "COLUMNS", "CARDS"],
                |b| {
                    vec![
                        b.id.clone(),
                        b.project_id.clone(),
                        b.name.clone(),
                        b.visibility.clone(),
                        b.columns_count.to_string(),
                        b.cards_count.to_string(),
                    ]
                },
                format,
            )
        }
        BoardAction::Get { board_id } => {
            let resp = client
                .get_board(tonic::Request::new(GetBoardRequest {
                    board_id: board_id.clone(),
                }))
                .await
                .with_ctx(|| "get board failed".to_string())?;
            render(
                &BoardDetailOut {
                    board: resp.board.map(BoardOut::from).unwrap_or(BoardOut {
                        id: board_id,
                        project_id: String::new(),
                        name: String::new(),
                        description: String::new(),
                        icon: String::new(),
                        created_at: String::new(),
                        updated_at: String::new(),
                        columns_count: 0,
                        cards_count: 0,
                        visibility: String::new(),
                    }),
                    columns: resp.columns.into_iter().map(Into::into).collect(),
                },
                format,
            )
        }
        BoardAction::Create {
            project,
            name,
            description,
            icon,
            visibility,
        } => {
            let req = CreateBoardRequest {
                project_id: project.clone(),
                name,
                description: description.unwrap_or_default(),
                icon: icon.unwrap_or_default(),
                idempotency_key: crate::kanban::new_idempotency_key(),
                visibility: visibility.into(),
            };
            let req = crate::kanban::client::request_with_object_id(req, &project)?;
            let resp = client
                .create_board(req)
                .await
                .with_ctx(|| "create board failed".to_string())?;
            render(&BoardOut::from(resp), format)
        }
        BoardAction::Update {
            board_id,
            name,
            description,
            icon,
            visibility,
        } => {
            let mut paths = Vec::new();
            if name.is_some() {
                paths.push("name".to_string());
            }
            if description.is_some() {
                paths.push("description".to_string());
            }
            if icon.is_some() {
                paths.push("icon".to_string());
            }
            if visibility.is_some() {
                paths.push("visibility".to_string());
            }
            let req = UpdateBoardRequest {
                board_id: board_id.clone(),
                board: Some(client::Board {
                    id: board_id.clone(),
                    name: name.unwrap_or_default(),
                    description: description.unwrap_or_default(),
                    icon: icon.unwrap_or_default(),
                    visibility: visibility.map(|v| v.into()).unwrap_or_default(),
                    ..Default::default()
                }),
                update_mask: Some(FieldMask { paths }),
            };
            let req = crate::kanban::client::request_with_object_id(req, &board_id)?;
            let resp = client
                .update_board(req)
                .await
                .with_ctx(|| "update board failed".to_string())?;
            render(&BoardOut::from(resp), format)
        }
        BoardAction::Delete { board_id } => {
            let req = crate::kanban::client::request_with_object_id(
                DeleteBoardRequest {
                    board_id: board_id.clone(),
                },
                &board_id,
            )?;
            client
                .delete_board(req)
                .await
                .with_ctx(|| "delete board failed".to_string())?;
            render(
                &serde_json::json!({"deleted": true, "board_id": board_id}),
                format,
            )
        }
        BoardAction::Column { action } => match action {
            ColumnAction::Add {
                board_id,
                title,
                accent,
                wip_limit,
                position,
            } => {
                let req = AddColumnRequest {
                    board_id: board_id.clone(),
                    title,
                    accent: accent.unwrap_or_default(),
                    wip_limit: wip_limit.unwrap_or_default(),
                    position: position.unwrap_or_default(),
                    idempotency_key: crate::kanban::new_idempotency_key(),
                };
                let req = crate::kanban::client::request_with_object_id(req, &board_id)?;
                let resp = client
                    .add_column(req)
                    .await
                    .with_ctx(|| "add column failed".to_string())?;
                render(&ColumnOut::from(resp), format)
            }
            ColumnAction::Update {
                board_id,
                column_id,
                title,
                accent,
                wip_limit,
            } => {
                let column_id = resolve_column_id(client, &board_id, &column_id).await?;
                let mut paths = Vec::new();
                if title.is_some() {
                    paths.push("title".to_string());
                }
                if accent.is_some() {
                    paths.push("accent".to_string());
                }
                if wip_limit.is_some() {
                    paths.push("wip_limit".to_string());
                }
                let req = UpdateColumnRequest {
                    board_id: board_id.clone(),
                    column_id: column_id.clone(),
                    column: Some(client::Column {
                        id: column_id.clone(),
                        board_id: board_id.clone(),
                        title: title.unwrap_or_default(),
                        accent: accent.unwrap_or_default(),
                        wip_limit: wip_limit.unwrap_or_default(),
                        ..Default::default()
                    }),
                    update_mask: Some(FieldMask { paths }),
                };
                let req = crate::kanban::client::request_with_object_id(req, &board_id)?;
                let resp = client
                    .update_column(req)
                    .await
                    .with_ctx(|| "update column failed".to_string())?;
                render(&ColumnOut::from(resp), format)
            }
            ColumnAction::Remove {
                board_id,
                column_id,
            } => {
                let column_id = resolve_column_id(client, &board_id, &column_id).await?;
                let req = crate::kanban::client::request_with_object_id(
                    RemoveColumnRequest {
                        board_id: board_id.clone(),
                        column_id: column_id.clone(),
                    },
                    &board_id,
                )?;
                client
                    .remove_column(req)
                    .await
                    .with_ctx(|| "remove column failed".to_string())?;
                render(
                    &serde_json::json!({
                        "removed": true,
                        "board_id": board_id,
                        "column_id": column_id,
                    }),
                    format,
                )
            }
            ColumnAction::Move {
                board_id,
                column_id,
                position,
            } => {
                let column_id = resolve_column_id(client, &board_id, &column_id).await?;
                let req = crate::kanban::client::request_with_object_id(
                    MoveColumnRequest {
                        board_id: board_id.clone(),
                        column_id: column_id.clone(),
                        to_position: position,
                    },
                    &board_id,
                )?;
                let resp = client
                    .move_column(req)
                    .await
                    .with_ctx(|| "move column failed".to_string())?;
                let columns: Vec<ColumnOut> = resp.columns.into_iter().map(Into::into).collect();
                render_list(
                    &columns,
                    &["ID", "TITLE", "POSITION", "WIP LIMIT"],
                    |c| {
                        vec![
                            c.id.clone(),
                            c.title.clone(),
                            c.position.to_string(),
                            c.wip_limit.to_string(),
                        ]
                    },
                    format,
                )
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::Timestamp;

    fn sample_board(id: &str) -> client::Board {
        client::Board {
            id: id.into(),
            project_id: "proj_1".into(),
            name: "Backlog".into(),
            description: "".into(),
            icon: "clipboard".into(),
            created_at: Some(Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            }),
            updated_at: Some(Timestamp {
                seconds: 1_700_000_100,
                nanos: 0,
            }),
            columns_count: 3,
            cards_count: 12,
            visibility: BoardVisibility::Private as i32,
        }
    }

    fn sample_column(id: &str, board_id: &str, position: i32) -> client::Column {
        client::Column {
            id: id.into(),
            board_id: board_id.into(),
            title: "To Do".into(),
            accent: "blue".into(),
            wip_limit: 5,
            position,
            created_at: Some(Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            }),
            updated_at: Some(Timestamp {
                seconds: 1_700_000_100,
                nanos: 0,
            }),
        }
    }

    #[tokio::test]
    async fn list_renders_boards() {
        let mut mock = MockBoardService::new();
        mock.expect_list_boards()
            .withf(|req| req.get_ref().project_id == "proj_1")
            .times(1)
            .returning(|_| {
                Ok(client::ListBoardsResponse {
                    boards: vec![sample_board("board_1")],
                })
            });

        run(
            BoardAction::List {
                project: "proj_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_renders_board_detail() {
        let mut mock = MockBoardService::new();
        mock.expect_get_board()
            .withf(|req| req.get_ref().board_id == "board_1")
            .times(1)
            .returning(|_| {
                Ok(client::BoardDetail {
                    board: Some(sample_board("board_1")),
                    columns: vec![sample_column("col_1", "board_1", 1)],
                })
            });

        run(
            BoardAction::Get {
                board_id: "board_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_renders_new_board() {
        let mut mock = MockBoardService::new();
        mock.expect_create_board()
            .withf(|req| {
                let r = req.get_ref();
                r.project_id == "proj_1"
                    && r.name == "Roadmap"
                    && r.visibility == BoardVisibility::Internal as i32
            })
            .times(1)
            .returning(|_| Ok(sample_board("board_new")));

        run(
            BoardAction::Create {
                project: "proj_1".into(),
                name: "Roadmap".into(),
                description: None,
                icon: None,
                visibility: VisibilityArg::Internal,
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_renders_updated_board() {
        let mut mock = MockBoardService::new();
        mock.expect_update_board()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1"
                    && r.board
                        .as_ref()
                        .map(|b| b.id == "board_1" && b.name == "Renamed")
                        .unwrap_or(false)
                    && r.update_mask
                        .as_ref()
                        .map(|m| m.paths == vec!["name"])
                        .unwrap_or(false)
                    && req
                        .metadata()
                        .get("x-sunbeam-object-id")
                        .and_then(|v| v.to_str().ok())
                        == Some("board_1")
            })
            .times(1)
            .returning(|_| {
                let mut b = sample_board("board_1");
                b.name = "Renamed".into();
                Ok(b)
            });

        run(
            BoardAction::Update {
                board_id: "board_1".into(),
                name: Some("Renamed".into()),
                description: None,
                icon: None,
                visibility: None,
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_succeeds() {
        let mut mock = MockBoardService::new();
        mock.expect_delete_board()
            .withf(|req| req.get_ref().board_id == "board_1")
            .times(1)
            .returning(|_| Ok(()));

        run(
            BoardAction::Delete {
                board_id: "board_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_add_renders_column() {
        let mut mock = MockBoardService::new();
        mock.expect_add_column()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1" && r.title == "Review"
            })
            .times(1)
            .returning(|_| Ok(sample_column("col_new", "board_1", 2)));

        run(
            BoardAction::Column {
                action: ColumnAction::Add {
                    board_id: "board_1".into(),
                    title: "Review".into(),
                    accent: None,
                    wip_limit: None,
                    position: None,
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_update_renders_column() {
        let mut mock = MockBoardService::new();
        mock.expect_update_column()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1"
                    && r.column_id == "col_1"
                    && r.column
                        .as_ref()
                        .map(|c| c.title == "Done")
                        .unwrap_or(false)
                    && r.update_mask
                        .as_ref()
                        .map(|m| m.paths == vec!["title"])
                        .unwrap_or(false)
            })
            .times(1)
            .returning(|_| Ok(sample_column("col_1", "board_1", 1)));

        run(
            BoardAction::Column {
                action: ColumnAction::Update {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                    title: Some("Done".into()),
                    accent: None,
                    wip_limit: None,
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_remove_succeeds() {
        let mut mock = MockBoardService::new();
        mock.expect_remove_column()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1" && r.column_id == "col_1"
            })
            .times(1)
            .returning(|_| Ok(()));

        run(
            BoardAction::Column {
                action: ColumnAction::Remove {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_move_renders_columns() {
        let mut mock = MockBoardService::new();
        mock.expect_move_column()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1" && r.column_id == "col_1" && r.to_position == 2
            })
            .times(1)
            .returning(|_| {
                Ok(client::MoveColumnResponse {
                    columns: vec![
                        sample_column("col_2", "board_1", 1),
                        sample_column("col_1", "board_1", 2),
                    ],
                })
            });

        run(
            BoardAction::Column {
                action: ColumnAction::Move {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                    position: 2,
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_boards_table_renders() {
        let mut mock = MockBoardService::new();
        mock.expect_list_boards().times(1).returning(|_| {
            Ok(client::ListBoardsResponse {
                boards: vec![sample_board("board_1")],
            })
        });

        run(
            BoardAction::List {
                project: "proj_1".into(),
            },
            OutputFormat::Table,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_board_without_board_uses_default() {
        let mut mock = MockBoardService::new();
        mock.expect_get_board().times(1).returning(|_| {
            Ok(client::BoardDetail {
                board: None,
                columns: vec![sample_column("col_1", "board_1", 1)],
            })
        });

        run(
            BoardAction::Get {
                board_id: "board_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_board_with_all_options_renders() {
        let mut mock = MockBoardService::new();
        mock.expect_create_board()
            .withf(|req| {
                let r = req.get_ref();
                r.project_id == "proj_1"
                    && r.name == "N"
                    && r.description == "D"
                    && r.icon == "I"
                    && r.visibility == BoardVisibility::Public as i32
                    && !r.idempotency_key.is_empty()
            })
            .times(1)
            .returning(|_| Ok(sample_board("board_new")));

        run(
            BoardAction::Create {
                project: "proj_1".into(),
                name: "N".into(),
                description: Some("D".into()),
                icon: Some("I".into()),
                visibility: VisibilityArg::Public,
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_board_with_all_options_renders() {
        let mut mock = MockBoardService::new();
        mock.expect_update_board()
            .withf(|req| {
                let r = req.get_ref();
                let paths = r.update_mask.as_ref().map(|m| m.paths.clone());
                paths
                    == Some(vec![
                        "name".into(),
                        "description".into(),
                        "icon".into(),
                        "visibility".into(),
                    ])
            })
            .times(1)
            .returning(|_| Ok(sample_board("board_1")));

        run(
            BoardAction::Update {
                board_id: "board_1".into(),
                name: Some("N".into()),
                description: Some("D".into()),
                icon: Some("I".into()),
                visibility: Some(VisibilityArg::Internal),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_add_with_all_options_renders() {
        let mut mock = MockBoardService::new();
        mock.expect_add_column()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1"
                    && r.title == "T"
                    && r.accent == "a"
                    && r.wip_limit == 5
                    && r.position == 3
                    && !r.idempotency_key.is_empty()
            })
            .times(1)
            .returning(|_| Ok(sample_column("col_new", "board_1", 3)));

        run(
            BoardAction::Column {
                action: ColumnAction::Add {
                    board_id: "board_1".into(),
                    title: "T".into(),
                    accent: Some("a".into()),
                    wip_limit: Some(5),
                    position: Some(3),
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_update_with_all_options_renders() {
        let mut mock = MockBoardService::new();
        mock.expect_update_column()
            .withf(|req| {
                let r = req.get_ref();
                let paths = r.update_mask.as_ref().map(|m| m.paths.clone());
                paths == Some(vec!["title".into(), "accent".into(), "wip_limit".into()])
            })
            .times(1)
            .returning(|_| Ok(sample_column("col_1", "board_1", 1)));

        run(
            BoardAction::Column {
                action: ColumnAction::Update {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                    title: Some("T".into()),
                    accent: Some("a".into()),
                    wip_limit: Some(5),
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[test]
    fn board_visibility_name_covers_default() {
        assert_eq!(
            board_visibility_name(BoardVisibility::Public as i32),
            "public"
        );
        assert_eq!(board_visibility_name(99), "unspecified");
    }

    #[tokio::test]
    async fn column_update_resolves_title() {
        let mut mock = MockBoardService::new();
        mock.expect_get_board()
            .withf(|req| req.get_ref().board_id == "board_1")
            .times(1)
            .returning(|_| {
                Ok(client::BoardDetail {
                    board: Some(sample_board("board_1")),
                    columns: vec![sample_column("col_1", "board_1", 1)],
                })
            });
        mock.expect_update_column()
            .withf(|req| {
                let r = req.get_ref();
                r.board_id == "board_1"
                    && r.column_id == "col_1"
                    && r.column
                        .as_ref()
                        .map(|c| c.title == "Renamed")
                        .unwrap_or(false)
            })
            .times(1)
            .returning(|_| Ok(sample_column("col_1", "board_1", 1)));

        run(
            BoardAction::Column {
                action: ColumnAction::Update {
                    board_id: "board_1".into(),
                    column_id: "To Do".into(),
                    title: Some("Renamed".into()),
                    accent: None,
                    wip_limit: None,
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn build_client_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build_client(&logger, ":::bad", "token").await.unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }
}
