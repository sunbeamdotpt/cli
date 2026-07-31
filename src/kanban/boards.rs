//! Kanban board commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, new_idempotency_key, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Board actions.
#[derive(Debug, Clone, Subcommand)]
pub enum BoardAction {
    /// List boards.
    List {
        /// Project ID, key, or name.
        project: String,
    },
    /// Get a board.
    Get {
        /// Board ID or name.
        board_id: String,
    },
    /// Create a board.
    Create {
        /// Project ID, key, or name.
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
        /// Visibility (defaults to private, with a warning).
        #[arg(long, value_enum)]
        visibility: Option<VisibilityArg>,
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
        #[arg(long, value_enum)]
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

impl From<VisibilityArg> for v1::BoardVisibility {
    fn from(v: VisibilityArg) -> Self {
        match v {
            VisibilityArg::Private => v1::BoardVisibility::Private,
            VisibilityArg::Internal => v1::BoardVisibility::Internal,
            VisibilityArg::Public => v1::BoardVisibility::Public,
        }
    }
}

/// Lowercase display name for a `BoardVisibility` value.
pub(crate) fn visibility_name(value: i32) -> String {
    match value {
        1 => "private".to_string(),
        2 => "internal".to_string(),
        3 => "public".to_string(),
        _ => "unspecified".to_string(),
    }
}

/// Board column actions.
#[derive(Debug, Clone, Subcommand)]
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
        /// Mark the column as a completion lane (moving a card in sets
        /// completed_at).
        #[arg(long)]
        is_done: bool,
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
        /// Mark the column as a completion lane.
        #[arg(long, conflicts_with = "no_is_done")]
        is_done: bool,
        /// Unmark the column as a completion lane.
        #[arg(long)]
        no_is_done: bool,
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

impl From<v1::Board> for BoardOut {
    fn from(b: v1::Board) -> Self {
        Self {
            id: b.id,
            project_id: b.project_id,
            name: b.name,
            description: b.description,
            icon: b.icon,
            created_at: fmt_ts(&b.created_at),
            updated_at: fmt_ts(&b.updated_at),
            columns_count: b.columns_count,
            cards_count: b.cards_count,
            visibility: visibility_name(b.visibility.to_i32()),
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
    is_done: bool,
    created_at: String,
    updated_at: String,
}

impl From<v1::Column> for ColumnOut {
    fn from(c: v1::Column) -> Self {
        Self {
            id: c.id,
            board_id: c.board_id,
            title: c.title,
            accent: c.accent,
            wip_limit: c.wip_limit,
            position: c.position,
            is_done: c.is_done,
            created_at: fmt_ts(&c.created_at),
            updated_at: fmt_ts(&c.updated_at),
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

/// Resolve a column identifier from a raw string within a board.
///
/// Thin wrapper over [`super::resolve::NameResolver::column`]; failures list
/// the board's columns (title + id).
async fn resolve_column_id(client: &KanbanClient, board_id: &str, raw: &str) -> Result<String> {
    super::resolve::NameResolver::new(client)
        .column(board_id, raw)
        .await
}

/// Run a board command.
pub(crate) async fn run(
    cmd: BoardAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        BoardAction::List { project } => {
            let resp = client
                .boards()
                .list_boards(v1::ListBoardsRequest {
                    project_id: project.clone(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            let boards: Vec<BoardOut> = resp.boards.into_iter().map(Into::into).collect();
            render_list(
                &boards,
                &["NAME", "VISIBILITY", "COLUMNS", "CARDS", "PROJECT ID", "ID"],
                |b| {
                    vec![
                        b.name.clone(),
                        b.visibility.clone(),
                        b.columns_count.to_string(),
                        b.cards_count.to_string(),
                        b.project_id.clone(),
                        b.id.clone(),
                    ]
                },
                format,
            )
        }
        BoardAction::Get { board_id } => {
            let resp = client
                .boards()
                .get_board(v1::GetBoardRequest {
                    board_id: board_id.clone(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            let detail = resp.detail.into_option().unwrap_or_default();
            let board = detail.board.into_option().map(BoardOut::from);
            render(
                &BoardDetailOut {
                    board: board.unwrap_or(BoardOut {
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
                    columns: detail.columns.into_iter().map(Into::into).collect(),
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
            let visibility = visibility.unwrap_or_else(|| {
                tracing::warn!(
                    "no --visibility given: creating a private board, invisible to the \
                     rest of the tenant; pass --visibility internal|public to share it"
                );
                VisibilityArg::Private
            });
            let resp = client
                .boards()
                .create_board_with_options(
                    v1::CreateBoardRequest {
                        project_id: project.clone(),
                        name,
                        description: description.unwrap_or_default(),
                        icon: icon.unwrap_or_default(),
                        idempotency_key: new_idempotency_key(),
                        visibility: v1::BoardVisibility::from(visibility).into(),
                        ..Default::default()
                    },
                    object_id_options(&project),
                )
                .await?
                .into_owned();
            render(&BoardOut::from(required(resp.board, "board")?), format)
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
            let resp = client
                .boards()
                .update_board_with_options(
                    v1::UpdateBoardRequest {
                        board_id: board_id.clone(),
                        board: v1::Board {
                            id: board_id.clone(),
                            name: name.unwrap_or_default(),
                            description: description.unwrap_or_default(),
                            icon: icon.unwrap_or_default(),
                            visibility: visibility
                                .map(v1::BoardVisibility::from)
                                .unwrap_or(v1::BoardVisibility::Unspecified)
                                .into(),
                            ..Default::default()
                        }
                        .into(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        ..Default::default()
                    },
                    object_id_options(&board_id),
                )
                .await?
                .into_owned();
            render(&BoardOut::from(required(resp.board, "board")?), format)
        }
        BoardAction::Delete { board_id } => {
            client
                .boards()
                .delete_board_with_options(
                    v1::DeleteBoardRequest {
                        board_id: board_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&board_id),
                )
                .await?;
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
                is_done,
            } => {
                let resp = client
                    .boards()
                    .add_column_with_options(
                        v1::AddColumnRequest {
                            board_id: board_id.clone(),
                            title,
                            accent: accent.unwrap_or_default(),
                            wip_limit: wip_limit.unwrap_or_default(),
                            position: position.unwrap_or_default(),
                            idempotency_key: new_idempotency_key(),
                            is_done,
                            ..Default::default()
                        },
                        object_id_options(&board_id),
                    )
                    .await?
                    .into_owned();
                render(&ColumnOut::from(required(resp.column, "column")?), format)
            }
            ColumnAction::Update {
                board_id,
                column_id,
                title,
                accent,
                wip_limit,
                is_done,
                no_is_done,
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
                if is_done || no_is_done {
                    paths.push("is_done".to_string());
                }
                let resp = client
                    .boards()
                    .update_column_with_options(
                        v1::UpdateColumnRequest {
                            board_id: board_id.clone(),
                            column_id: column_id.clone(),
                            column: v1::Column {
                                id: column_id.clone(),
                                board_id: board_id.clone(),
                                title: title.unwrap_or_default(),
                                accent: accent.unwrap_or_default(),
                                wip_limit: wip_limit.unwrap_or_default(),
                                is_done,
                                ..Default::default()
                            }
                            .into(),
                            update_mask: Some(buffa_types::google::protobuf::FieldMask {
                                paths,
                                ..Default::default()
                            })
                            .into(),
                            ..Default::default()
                        },
                        object_id_options(&board_id),
                    )
                    .await?
                    .into_owned();
                render(&ColumnOut::from(required(resp.column, "column")?), format)
            }
            ColumnAction::Remove {
                board_id,
                column_id,
            } => {
                let column_id = resolve_column_id(client, &board_id, &column_id).await?;
                client
                    .boards()
                    .remove_column_with_options(
                        v1::RemoveColumnRequest {
                            board_id: board_id.clone(),
                            column_id: column_id.clone(),
                            ..Default::default()
                        },
                        object_id_options(&board_id),
                    )
                    .await?;
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
                let resp = client
                    .boards()
                    .move_column_with_options(
                        v1::MoveColumnRequest {
                            board_id: board_id.clone(),
                            column_id: column_id.clone(),
                            to_position: position,
                            ..Default::default()
                        },
                        object_id_options(&board_id),
                    )
                    .await?
                    .into_owned();
                let columns: Vec<ColumnOut> = resp.columns.into_iter().map(Into::into).collect();
                render_list(
                    &columns,
                    &["TITLE", "POSITION", "WIP LIMIT", "ID"],
                    |c| {
                        vec![
                            c.title.clone(),
                            c.position.to_string(),
                            c.wip_limit.to_string(),
                            c.id.clone(),
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
    use crate::kanban::testutil;
    use sdk::kanban::prelude::buffa;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn board(id: &str, name: &str) -> v1::Board {
        v1::Board {
            id: id.to_string(),
            project_id: "proj_1".into(),
            name: name.to_string(),
            columns_count: 3,
            cards_count: 12,
            visibility: v1::BoardVisibility::Public.into(),
            ..Default::default()
        }
    }

    fn column(id: &str, title: &str) -> v1::Column {
        v1::Column {
            id: id.to_string(),
            board_id: "board_1".into(),
            title: title.to_string(),
            position: 1,
            ..Default::default()
        }
    }

    fn board_detail() -> v1::BoardDetail {
        v1::BoardDetail {
            board: board("board_1", "Backlog").into(),
            columns: vec![column("col_1", "Todo")],
            ..Default::default()
        }
    }

    #[test]
    fn visibility_arg_maps_to_proto() {
        assert_eq!(
            buffa::EnumValue::from(v1::BoardVisibility::from(VisibilityArg::Private)).to_i32(),
            1
        );
        assert_eq!(
            buffa::EnumValue::from(v1::BoardVisibility::from(VisibilityArg::Internal)).to_i32(),
            2
        );
        assert_eq!(
            buffa::EnumValue::from(v1::BoardVisibility::from(VisibilityArg::Public)).to_i32(),
            3
        );
    }

    #[test]
    fn visibility_name_labels() {
        assert_eq!(visibility_name(1), "private");
        assert_eq!(visibility_name(2), "internal");
        assert_eq!(visibility_name(3), "public");
        assert_eq!(visibility_name(0), "unspecified");
        assert_eq!(visibility_name(99), "unspecified");
    }

    #[tokio::test]
    async fn list_boards_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.BoardService/ListBoards"))
                .respond_with(testutil::proto_response(&v1::ListBoardsResponse {
                    boards: vec![board("board_1", "Backlog")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                BoardAction::List {
                    project: "proj_1".into(),
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn get_board_renders_detail() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse {
                detail: board_detail().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            BoardAction::Get {
                board_id: "board_1".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_board_without_board_field() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse::default()))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            BoardAction::Get {
                board_id: "board_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_update_delete_board() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/CreateBoard"))
            .respond_with(testutil::proto_response(&v1::CreateBoardResponse {
                board: board("board_new", "New").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/UpdateBoard"))
            .respond_with(testutil::proto_response(&v1::UpdateBoardResponse {
                board: board("board_1", "Renamed").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/DeleteBoard"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            BoardAction::Create {
                project: "proj_1".into(),
                name: "New".into(),
                description: None,
                icon: Some("star".into()),
                visibility: Some(VisibilityArg::Internal),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        // No --visibility: defaults to private (with a warning).
        run(
            BoardAction::Create {
                project: "proj_1".into(),
                name: "Quiet".into(),
                description: None,
                icon: None,
                visibility: None,
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            BoardAction::Update {
                board_id: "board_1".into(),
                name: Some("Renamed".into()),
                description: Some("d".into()),
                icon: None,
                visibility: Some(VisibilityArg::Public),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            BoardAction::Delete {
                board_id: "board_1".into(),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_add_update_remove_move() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/AddColumn"))
            .respond_with(testutil::proto_response(&v1::AddColumnResponse {
                column: column("col_1", "Todo").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/UpdateColumn"))
            .respond_with(testutil::proto_response(&v1::UpdateColumnResponse {
                column: column("col_1", "Doing").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/RemoveColumn"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/MoveColumn"))
            .respond_with(testutil::proto_response(&v1::MoveColumnResponse {
                columns: vec![column("col_1", "Todo"), column("col_2", "Done")],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse {
                detail: board_detail().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            BoardAction::Column {
                action: ColumnAction::Add {
                    board_id: "board_1".into(),
                    title: "Todo".into(),
                    accent: Some("blue".into()),
                    wip_limit: Some(5),
                    position: None,
                    is_done: false,
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            BoardAction::Column {
                action: ColumnAction::Update {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                    title: Some("Doing".into()),
                    accent: None,
                    wip_limit: Some(3),
                    is_done: false,
                    no_is_done: false,
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            BoardAction::Column {
                action: ColumnAction::Remove {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                },
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            BoardAction::Column {
                action: ColumnAction::Move {
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                    position: 2,
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn column_update_resolves_title_to_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse {
                detail: board_detail().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/UpdateColumn"))
            .respond_with(testutil::proto_response(&v1::UpdateColumnResponse {
                column: column("col_1", "Doing").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            BoardAction::Column {
                action: ColumnAction::Update {
                    board_id: "board_1".into(),
                    column_id: "todo".into(),
                    title: Some("Doing".into()),
                    accent: None,
                    wip_limit: None,
                    is_done: false,
                    no_is_done: false,
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[test]
    fn column_out_surfaces_is_done() {
        let out = ColumnOut::from(v1::Column {
            is_done: true,
            ..column("col_1", "Done")
        });
        assert!(out.is_done);
        assert!(!ColumnOut::from(column("col_2", "Todo")).is_done);
    }

    #[tokio::test]
    async fn column_add_is_done_sets_request_flag() {
        use sdk::kanban::prelude::buffa::Message;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/AddColumn"))
            .respond_with(testutil::proto_response(&v1::AddColumnResponse {
                column: v1::Column {
                    is_done: true,
                    ..column("col_1", "Done")
                }
                .into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            BoardAction::Column {
                action: ColumnAction::Add {
                    board_id: "board_1".into(),
                    title: "Done".into(),
                    accent: None,
                    wip_limit: None,
                    position: None,
                    is_done: true,
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path().ends_with("/AddColumn"))
            .expect("AddColumn request");
        let decoded = v1::AddColumnRequest::decode(&mut req.body.as_slice()).unwrap();
        assert!(decoded.is_done);
    }

    #[tokio::test]
    async fn column_update_is_done_sends_update_mask() {
        use sdk::kanban::prelude::buffa::Message;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/UpdateColumn"))
            .respond_with(testutil::proto_response(&v1::UpdateColumnResponse {
                column: column("col_1", "Done").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        for (is_done, no_is_done, want) in [(true, false, true), (false, true, false)] {
            run(
                BoardAction::Column {
                    action: ColumnAction::Update {
                        board_id: "board_1".into(),
                        column_id: "col_1".into(),
                        title: None,
                        accent: None,
                        wip_limit: None,
                        is_done,
                        no_is_done,
                    },
                },
                OutputFormat::Json,
                &client,
            )
            .await
            .unwrap();

            let requests = server.received_requests().await.unwrap();
            let req = requests
                .iter()
                .rfind(|r| r.url.path().ends_with("/UpdateColumn"))
                .expect("UpdateColumn request");
            let decoded = v1::UpdateColumnRequest::decode(&mut req.body.as_slice()).unwrap();
            let mask = decoded.update_mask.as_option().expect("update_mask");
            assert_eq!(mask.paths, vec!["is_done".to_string()]);
            assert_eq!(decoded.column.as_option().unwrap().is_done, want);
        }
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/ListBoards"))
            .respond_with(testutil::connect_error(
                403,
                "permission_denied",
                "no view relation",
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            BoardAction::List {
                project: "proj_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no view relation"));
    }
}
