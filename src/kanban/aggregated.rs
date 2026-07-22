//! Kanban aggregated (meta) board commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use sdk::kanban::v1::__buffa::oneof::aggregated_board_chunk::Payload as ChunkPayload;
use serde::Serialize;

use super::boards::{VisibilityArg, visibility_name};
use super::{fmt_ts, new_idempotency_key, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Aggregated board actions.
#[derive(Debug, Subcommand)]
pub enum AggregateAction {
    /// List aggregated boards.
    List,
    /// Get an aggregated board.
    Get {
        /// Aggregated board ID or name.
        aggregate_id: String,
    },
    /// Create an aggregated board.
    Create {
        /// Name.
        #[arg(short, long)]
        name: String,
        /// Description.
        #[arg(short, long)]
        description: Option<String>,
        /// Icon.
        #[arg(short, long)]
        icon: Option<String>,
        /// Visibility.
        #[arg(long, value_enum)]
        visibility: Option<VisibilityArg>,
    },
    /// Update an aggregated board.
    Update {
        /// Aggregated board ID or name.
        aggregate_id: String,
        /// New name.
        #[arg(short, long)]
        name: Option<String>,
        /// New description.
        #[arg(short, long)]
        description: Option<String>,
        /// New icon.
        #[arg(short, long)]
        icon: Option<String>,
    },
    /// Delete an aggregated board.
    Delete {
        /// Aggregated board ID or name.
        aggregate_id: String,
    },
    /// Source board management.
    Source {
        /// Source board subcommand to run.
        #[command(subcommand)]
        action: SourceAction,
    },
}

/// Source board actions.
#[derive(Debug, Subcommand)]
pub enum SourceAction {
    /// Add a source board.
    Add {
        /// Aggregated board ID or name.
        aggregate_id: String,
        /// Source board ID or name.
        board_id: String,
        /// Display position.
        #[arg(short, long)]
        position: Option<i32>,
    },
    /// Remove a source board.
    Remove {
        /// Aggregated board ID or name.
        aggregate_id: String,
        /// Source board ID or name.
        board_id: String,
    },
    /// Move a source board.
    Move {
        /// Aggregated board ID or name.
        aggregate_id: String,
        /// Source board ID or name.
        board_id: String,
        /// New position.
        #[arg(short, long)]
        position: i32,
    },
}

/// Serializable aggregated board summary for list views.
#[derive(Serialize)]
struct AggregatedBoardOut {
    id: String,
    name: String,
    description: String,
    icon: String,
    visibility: String,
    created_at: String,
    updated_at: String,
}

impl From<v1::AggregatedBoard> for AggregatedBoardOut {
    fn from(board: v1::AggregatedBoard) -> Self {
        Self {
            id: board.id,
            name: board.name,
            description: board.description,
            icon: board.icon,
            visibility: visibility_name(board.visibility.to_i32()),
            created_at: fmt_ts(&board.created_at),
            updated_at: fmt_ts(&board.updated_at),
        }
    }
}

fn source_to_json(s: v1::SourceBoardRef) -> serde_json::Value {
    serde_json::json!({
        "board_id": s.board_id,
        "project_id": s.project_id,
        "name": s.name,
        "icon": s.icon,
        "position": s.position,
    })
}

fn column_to_json(c: v1::AggregatedColumn) -> serde_json::Value {
    serde_json::json!({
        "id": c.id,
        "title": c.title,
        "accent": c.accent,
        "wip_limit": c.wip_limit,
        "position": c.position,
        "source_board_id": c.source_board_id,
    })
}

/// Assemble the streamed chunks of a GetAggregatedBoard call into one document.
fn assemble_detail(chunks: Vec<v1::AggregatedBoardChunk>) -> serde_json::Value {
    let mut metadata: Option<v1::AggregatedBoard> = None;
    let mut sources: Vec<v1::SourceBoardRef> = Vec::new();
    let mut columns: Vec<v1::AggregatedColumn> = Vec::new();
    let mut cards: Vec<v1::Card> = Vec::new();

    for chunk in chunks {
        match chunk.payload {
            Some(ChunkPayload::Metadata(m)) => metadata = Some(*m),
            Some(ChunkPayload::SourceBoard(s)) => sources.push(*s),
            Some(ChunkPayload::Column(c)) => columns.push(*c),
            Some(ChunkPayload::CardBatch(b)) => cards.extend(b.cards),
            None => {}
        }
    }

    serde_json::json!({
        "metadata": metadata.map(AggregatedBoardOut::from),
        "sources": sources.into_iter().map(source_to_json).collect::<Vec<_>>(),
        "columns": columns.into_iter().map(column_to_json).collect::<Vec<_>>(),
        "cards": cards.into_iter().map(super::cards::CardDetailOut::from).collect::<Vec<_>>(),
    })
}

/// Run an aggregated board command.
pub(crate) async fn run(
    cmd: AggregateAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        AggregateAction::List => {
            let resp = client
                .aggregated_boards()
                .list_aggregated_boards(v1::ListAggregatedBoardsRequest::default())
                .await?
                .into_owned();
            let boards: Vec<_> = resp
                .aggregated_boards
                .into_iter()
                .map(AggregatedBoardOut::from)
                .collect();
            render_list(
                &boards,
                &["NAME", "DESCRIPTION", "ICON", "VISIBILITY", "ID"],
                |b| {
                    vec![
                        b.name.clone(),
                        b.description.clone(),
                        b.icon.clone(),
                        b.visibility.clone(),
                        b.id.clone(),
                    ]
                },
                format,
            )
        }
        AggregateAction::Get { aggregate_id } => {
            let aggregate_id = super::resolve::NameResolver::new(client)
                .aggregate(&aggregate_id)
                .await?;
            let mut stream = client
                .aggregated_boards()
                .get_aggregated_board(v1::GetAggregatedBoardRequest {
                    aggregated_board_id: aggregate_id.clone(),
                    ..Default::default()
                })
                .await?;
            let mut chunks = Vec::new();
            while let Some(resp) = stream.message().await? {
                if let Some(chunk) = resp.to_owned_message().chunk.into_option() {
                    chunks.push(chunk);
                }
            }
            render(&assemble_detail(chunks), format)
        }
        AggregateAction::Create {
            name,
            description,
            icon,
            visibility,
        } => {
            let resp = client
                .aggregated_boards()
                .create_aggregated_board(v1::CreateAggregatedBoardRequest {
                    name,
                    description: description.unwrap_or_default(),
                    icon: icon.unwrap_or_default(),
                    source_board_ids: Vec::new(),
                    idempotency_key: new_idempotency_key(),
                    visibility: visibility
                        .map(v1::BoardVisibility::from)
                        .unwrap_or(v1::BoardVisibility::Private)
                        .into(),
                    ..Default::default()
                })
                .await?
                .into_owned();
            render(
                &AggregatedBoardOut::from(required(resp.aggregated_board, "aggregated board")?),
                format,
            )
        }
        AggregateAction::Update {
            aggregate_id,
            name,
            description,
            icon,
        } => {
            let aggregate_id = super::resolve::NameResolver::new(client)
                .aggregate(&aggregate_id)
                .await?;
            let mut board = v1::AggregatedBoard {
                id: aggregate_id.clone(),
                ..Default::default()
            };
            let mut paths: Vec<String> = Vec::new();
            if let Some(n) = name {
                board.name = n;
                paths.push("name".to_string());
            }
            if let Some(d) = description {
                board.description = d;
                paths.push("description".to_string());
            }
            if let Some(i) = icon {
                board.icon = i;
                paths.push("icon".to_string());
            }
            let resp = client
                .aggregated_boards()
                .update_aggregated_board_with_options(
                    v1::UpdateAggregatedBoardRequest {
                        aggregated_board_id: aggregate_id.clone(),
                        aggregated_board: board.into(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        ..Default::default()
                    },
                    object_id_options(&aggregate_id),
                )
                .await?
                .into_owned();
            render(
                &AggregatedBoardOut::from(required(resp.aggregated_board, "aggregated board")?),
                format,
            )
        }
        AggregateAction::Delete { aggregate_id } => {
            let aggregate_id = super::resolve::NameResolver::new(client)
                .aggregate(&aggregate_id)
                .await?;
            client
                .aggregated_boards()
                .delete_aggregated_board_with_options(
                    v1::DeleteAggregatedBoardRequest {
                        aggregated_board_id: aggregate_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&aggregate_id),
                )
                .await?;
            Ok(())
        }
        AggregateAction::Source { action } => match action {
            SourceAction::Add {
                aggregate_id,
                board_id,
                position,
            } => {
                let aggregate_id = super::resolve::NameResolver::new(client)
                    .aggregate(&aggregate_id)
                    .await?;
                let resp = client
                    .aggregated_boards()
                    .add_source_board_with_options(
                        v1::AddSourceBoardRequest {
                            aggregated_board_id: aggregate_id.clone(),
                            board_id: board_id.clone(),
                            position: position.unwrap_or_default(),
                            ..Default::default()
                        },
                        object_id_options(&aggregate_id),
                    )
                    .await?
                    .into_owned();
                render(
                    &AggregatedBoardOut::from(required(resp.aggregated_board, "aggregated board")?),
                    format,
                )
            }
            SourceAction::Remove {
                aggregate_id,
                board_id,
            } => {
                let aggregate_id = super::resolve::NameResolver::new(client)
                    .aggregate(&aggregate_id)
                    .await?;
                let resp = client
                    .aggregated_boards()
                    .remove_source_board_with_options(
                        v1::RemoveSourceBoardRequest {
                            aggregated_board_id: aggregate_id.clone(),
                            board_id: board_id.clone(),
                            ..Default::default()
                        },
                        object_id_options(&aggregate_id),
                    )
                    .await?
                    .into_owned();
                render(
                    &AggregatedBoardOut::from(required(resp.aggregated_board, "aggregated board")?),
                    format,
                )
            }
            SourceAction::Move {
                aggregate_id,
                board_id,
                position,
            } => {
                let aggregate_id = super::resolve::NameResolver::new(client)
                    .aggregate(&aggregate_id)
                    .await?;
                let resp = client
                    .aggregated_boards()
                    .move_source_board_with_options(
                        v1::MoveSourceBoardRequest {
                            aggregated_board_id: aggregate_id.clone(),
                            board_id: board_id.clone(),
                            to_position: position,
                            ..Default::default()
                        },
                        object_id_options(&aggregate_id),
                    )
                    .await?
                    .into_owned();
                render(
                    &AggregatedBoardOut::from(required(resp.aggregated_board, "aggregated board")?),
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
    use sdk::kanban::prelude::buffa::Message;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn agg(id: &str, name: &str) -> v1::AggregatedBoard {
        v1::AggregatedBoard {
            id: id.to_string(),
            name: name.to_string(),
            description: "meta".into(),
            icon: "star".into(),
            visibility: v1::BoardVisibility::Internal.into(),
            ..Default::default()
        }
    }

    #[test]
    fn assemble_detail_groups_chunks() {
        let chunks = vec![
            v1::AggregatedBoardChunk {
                payload: Some(ChunkPayload::Metadata(Box::new(agg("agg_1", "Meta")))),
                ..Default::default()
            },
            v1::AggregatedBoardChunk {
                payload: Some(ChunkPayload::SourceBoard(Box::new(v1::SourceBoardRef {
                    board_id: "board_1".into(),
                    project_id: "proj_1".into(),
                    name: "Backlog".into(),
                    position: 1,
                    ..Default::default()
                }))),
                ..Default::default()
            },
            v1::AggregatedBoardChunk {
                payload: Some(ChunkPayload::Column(Box::new(v1::AggregatedColumn {
                    id: "acol_1".into(),
                    title: "Backlog".into(),
                    position: 1,
                    source_board_id: "board_1".into(),
                    ..Default::default()
                }))),
                ..Default::default()
            },
            v1::AggregatedBoardChunk {
                payload: Some(ChunkPayload::CardBatch(Box::new(v1::AggregatedCardBatch {
                    cards: vec![v1::Card {
                        id: "card_1".into(),
                        title: "Fix crash".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }))),
                ..Default::default()
            },
            v1::AggregatedBoardChunk::default(),
        ];
        let out = assemble_detail(chunks);
        assert_eq!(out["metadata"]["name"], "Meta");
        assert_eq!(out["sources"][0]["board_id"], "board_1");
        assert_eq!(out["columns"][0]["source_board_id"], "board_1");
        assert_eq!(out["cards"][0]["title"], "Fix crash");
    }

    /// Build a Connect streaming response body: data envelopes + end stream.
    fn connect_stream_body(payloads: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        for p in payloads {
            body.push(0u8); // flags: data
            body.extend_from_slice(&(p.len() as u32).to_be_bytes());
            body.extend_from_slice(p);
        }
        let end = b"{}";
        body.push(2u8); // flags: end-of-stream
        body.extend_from_slice(&(end.len() as u32).to_be_bytes());
        body.extend_from_slice(end);
        body
    }

    #[tokio::test]
    async fn list_aggregated_boards_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(
                    "/sunbeam.kanban.v1.AggregatedBoardService/ListAggregatedBoards",
                ))
                .respond_with(testutil::proto_response(
                    &v1::ListAggregatedBoardsResponse {
                        aggregated_boards: vec![agg("agg_1", "Meta")],
                        ..Default::default()
                    },
                ))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(AggregateAction::List, format, &client).await.unwrap();
        }
    }

    #[tokio::test]
    async fn get_aggregated_board_streams_chunks() {
        let server = MockServer::start().await;
        let chunks = [
            v1::AggregatedBoardChunk {
                payload: Some(ChunkPayload::Metadata(Box::new(agg("agg_1", "Meta")))),
                ..Default::default()
            },
            v1::AggregatedBoardChunk {
                payload: Some(ChunkPayload::CardBatch(Box::new(v1::AggregatedCardBatch {
                    cards: vec![v1::Card {
                        id: "card_1".into(),
                        title: "Fix crash".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }))),
                ..Default::default()
            },
        ]
        .into_iter()
        .map(|c| {
            v1::GetAggregatedBoardResponse {
                chunk: c.into(),
                ..Default::default()
            }
            .encode_to_vec()
        })
        .collect::<Vec<_>>();
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AggregatedBoardService/GetAggregatedBoard",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+proto")
                    .set_body_bytes(connect_stream_body(&chunks)),
            )
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            AggregateAction::Get {
                aggregate_id: "agg_1".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_update_delete_aggregate() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AggregatedBoardService/CreateAggregatedBoard",
            ))
            .respond_with(testutil::proto_response(
                &v1::CreateAggregatedBoardResponse {
                    aggregated_board: agg("agg_new", "New").into(),
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AggregatedBoardService/UpdateAggregatedBoard",
            ))
            .respond_with(testutil::proto_response(
                &v1::UpdateAggregatedBoardResponse {
                    aggregated_board: agg("agg_1", "Renamed").into(),
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AggregatedBoardService/DeleteAggregatedBoard",
            ))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            AggregateAction::Create {
                name: "New".into(),
                description: Some("d".into()),
                icon: None,
                visibility: Some(VisibilityArg::Public),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            AggregateAction::Update {
                aggregate_id: "agg_1".into(),
                name: Some("Renamed".into()),
                description: None,
                icon: Some("bolt".into()),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            AggregateAction::Delete {
                aggregate_id: "agg_1".into(),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn source_add_remove_move() {
        let server = MockServer::start().await;
        for (rpc, body) in [
            (
                "AddSourceBoard",
                testutil::proto_response(&v1::AddSourceBoardResponse {
                    aggregated_board: agg("agg_1", "Meta").into(),
                    ..Default::default()
                }),
            ),
            (
                "RemoveSourceBoard",
                testutil::proto_response(&v1::RemoveSourceBoardResponse {
                    aggregated_board: agg("agg_1", "Meta").into(),
                    ..Default::default()
                }),
            ),
            (
                "MoveSourceBoard",
                testutil::proto_response(&v1::MoveSourceBoardResponse {
                    aggregated_board: agg("agg_1", "Meta").into(),
                    ..Default::default()
                }),
            ),
        ] {
            Mock::given(method("POST"))
                .and(path(format!(
                    "/sunbeam.kanban.v1.AggregatedBoardService/{rpc}"
                )))
                .respond_with(body)
                .mount(&server)
                .await;
        }

        let client = testutil::client_for(&server.uri());
        run(
            AggregateAction::Source {
                action: SourceAction::Add {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                    position: Some(2),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            AggregateAction::Source {
                action: SourceAction::Remove {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            AggregateAction::Source {
                action: SourceAction::Move {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                    position: 1,
                },
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AggregatedBoardService/ListAggregatedBoards",
            ))
            .respond_with(testutil::connect_error(500, "internal", "boom"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(AggregateAction::List, OutputFormat::Table, &client)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("boom"));
    }
}
