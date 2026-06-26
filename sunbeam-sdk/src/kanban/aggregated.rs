//! Kanban aggregated (meta) board commands.

use crate::error::{Result, ResultExt};
use crate::kanban::boards::VisibilityArg;
use crate::kanban::client::{self, AggregatedBoardServiceClient, request_with_object_id};
use crate::kanban::new_idempotency_key;
use crate::logger::Logger;
use crate::output::{OutputFormat, render, render_list};
use async_trait::async_trait;
use clap::Subcommand;
use serde::Serialize;

/// Aggregated board actions.
#[derive(Debug, Subcommand)]
pub enum AggregateAction {
    /// List aggregated boards.
    List,
    /// Get an aggregated board.
    Get {
        /// Aggregated board ID.
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
        #[arg(short, long, value_enum)]
        visibility: Option<VisibilityArg>,
    },
    /// Update an aggregated board.
    Update {
        /// Aggregated board ID.
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
        /// Aggregated board ID.
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
        /// Aggregated board ID.
        aggregate_id: String,
        /// Source board ID.
        board_id: String,
        /// Display position.
        #[arg(short, long)]
        position: Option<i32>,
    },
    /// Remove a source board.
    Remove {
        /// Aggregated board ID.
        aggregate_id: String,
        /// Source board ID.
        board_id: String,
    },
    /// Move a source board.
    Move {
        /// Aggregated board ID.
        aggregate_id: String,
        /// Source board ID.
        board_id: String,
        /// New position.
        #[arg(short, long)]
        position: i32,
    },
}

impl VisibilityArg {
    /// Convert to the generated proto enum value.
    fn to_proto(self) -> client::BoardVisibility {
        match self {
            VisibilityArg::Private => client::BoardVisibility::Private,
            VisibilityArg::Internal => client::BoardVisibility::Internal,
            VisibilityArg::Public => client::BoardVisibility::Public,
        }
    }
}

/// Trait abstracting the Kanban aggregated board service for testability.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AggregatedBoardService {
    /// List aggregated boards.
    async fn list_aggregated_boards(
        &mut self,
        req: tonic::Request<client::ListAggregatedBoardsRequest>,
    ) -> Result<client::ListAggregatedBoardsResponse>;

    /// Get an aggregated board as a stream of chunks.
    async fn get_aggregated_board(
        &mut self,
        req: tonic::Request<client::GetAggregatedBoardRequest>,
    ) -> Result<Vec<client::AggregatedBoardChunk>>;

    /// Create an aggregated board.
    async fn create_aggregated_board(
        &mut self,
        req: tonic::Request<client::CreateAggregatedBoardRequest>,
    ) -> Result<client::AggregatedBoard>;

    /// Update an aggregated board.
    async fn update_aggregated_board(
        &mut self,
        req: tonic::Request<client::UpdateAggregatedBoardRequest>,
    ) -> Result<client::AggregatedBoard>;

    /// Delete an aggregated board.
    async fn delete_aggregated_board(
        &mut self,
        req: tonic::Request<client::DeleteAggregatedBoardRequest>,
    ) -> Result<()>;

    /// Add a source board to an aggregate.
    async fn add_source_board(
        &mut self,
        req: tonic::Request<client::AddSourceBoardRequest>,
    ) -> Result<client::AggregatedBoard>;

    /// Remove a source board from an aggregate.
    async fn remove_source_board(
        &mut self,
        req: tonic::Request<client::RemoveSourceBoardRequest>,
    ) -> Result<client::AggregatedBoard>;

    /// Move a source board within an aggregate.
    async fn move_source_board(
        &mut self,
        req: tonic::Request<client::MoveSourceBoardRequest>,
    ) -> Result<client::AggregatedBoard>;
}

/// Wrapper around the generated Tonic aggregated-board client.
#[derive(Debug)]
pub struct AggregatedBoardServiceClientWrapper {
    inner: AggregatedBoardServiceClient<client::AuthChannel>,
}

impl AggregatedBoardServiceClientWrapper {
    /// Build a wrapper from an authenticated channel.
    pub fn new(inner: AggregatedBoardServiceClient<client::AuthChannel>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AggregatedBoardService for AggregatedBoardServiceClientWrapper {
    async fn list_aggregated_boards(
        &mut self,
        req: tonic::Request<client::ListAggregatedBoardsRequest>,
    ) -> Result<client::ListAggregatedBoardsResponse> {
        Ok(self.inner.list_aggregated_boards(req).await?.into_inner())
    }

    async fn get_aggregated_board(
        &mut self,
        req: tonic::Request<client::GetAggregatedBoardRequest>,
    ) -> Result<Vec<client::AggregatedBoardChunk>> {
        let mut stream = self.inner.get_aggregated_board(req).await?.into_inner();
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.message().await? {
            chunks.push(chunk);
        }
        Ok(chunks)
    }

    async fn create_aggregated_board(
        &mut self,
        req: tonic::Request<client::CreateAggregatedBoardRequest>,
    ) -> Result<client::AggregatedBoard> {
        Ok(self.inner.create_aggregated_board(req).await?.into_inner())
    }

    async fn update_aggregated_board(
        &mut self,
        req: tonic::Request<client::UpdateAggregatedBoardRequest>,
    ) -> Result<client::AggregatedBoard> {
        Ok(self.inner.update_aggregated_board(req).await?.into_inner())
    }

    async fn delete_aggregated_board(
        &mut self,
        req: tonic::Request<client::DeleteAggregatedBoardRequest>,
    ) -> Result<()> {
        self.inner.delete_aggregated_board(req).await?;
        Ok(())
    }

    async fn add_source_board(
        &mut self,
        req: tonic::Request<client::AddSourceBoardRequest>,
    ) -> Result<client::AggregatedBoard> {
        Ok(self.inner.add_source_board(req).await?.into_inner())
    }

    async fn remove_source_board(
        &mut self,
        req: tonic::Request<client::RemoveSourceBoardRequest>,
    ) -> Result<client::AggregatedBoard> {
        Ok(self.inner.remove_source_board(req).await?.into_inner())
    }

    async fn move_source_board(
        &mut self,
        req: tonic::Request<client::MoveSourceBoardRequest>,
    ) -> Result<client::AggregatedBoard> {
        Ok(self.inner.move_source_board(req).await?.into_inner())
    }
}

/// Build an aggregated-board service client wrapper for the given server and token.
pub async fn build_client(
    logger: &Logger,
    server: &str,
    token: &str,
) -> Result<AggregatedBoardServiceClientWrapper> {
    let channel = client::build(logger, server, token).await?;
    Ok(AggregatedBoardServiceClientWrapper::new(
        AggregatedBoardServiceClient::new(channel),
    ))
}

/// Convert a prost Timestamp to an RFC3339 string.
fn fmt_ts(ts: Option<&prost_types::Timestamp>) -> String {
    ts.and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos as u32))
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
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

impl AggregatedBoardOut {
    fn from_proto(board: client::AggregatedBoard) -> Self {
        Self {
            id: board.id,
            name: board.name,
            description: board.description,
            icon: board.icon,
            visibility: client::BoardVisibility::try_from(board.visibility)
                .map(|v| v.as_str_name().to_string())
                .unwrap_or_default(),
            created_at: fmt_ts(board.created_at.as_ref()),
            updated_at: fmt_ts(board.updated_at.as_ref()),
        }
    }
}

fn source_to_json(s: client::SourceBoardRef) -> serde_json::Value {
    serde_json::json!({
        "board_id": s.board_id,
        "project_id": s.project_id,
        "name": s.name,
        "icon": s.icon,
        "position": s.position,
    })
}

fn column_to_json(c: client::AggregatedColumn) -> serde_json::Value {
    serde_json::json!({
        "id": c.id,
        "title": c.title,
        "accent": c.accent,
        "wip_limit": c.wip_limit,
        "position": c.position,
        "source_board_id": c.source_board_id,
    })
}

/// Run an aggregated board command.
pub async fn run(
    cmd: AggregateAction,
    format: OutputFormat,
    client: &mut dyn AggregatedBoardService,
) -> Result<()> {
    match cmd {
        AggregateAction::List => {
            let resp = client
                .list_aggregated_boards(tonic::Request::new(client::ListAggregatedBoardsRequest {}))
                .await
                .with_ctx(|| "list aggregated boards".to_string())?;
            let boards: Vec<_> = resp
                .aggregated_boards
                .into_iter()
                .map(AggregatedBoardOut::from_proto)
                .collect();
            render_list(
                &boards,
                &["ID", "NAME", "DESCRIPTION", "ICON", "VISIBILITY"],
                |b| {
                    vec![
                        b.id.clone(),
                        b.name.clone(),
                        b.description.clone(),
                        b.icon.clone(),
                        b.visibility.clone(),
                    ]
                },
                format,
            )
        }
        AggregateAction::Get { aggregate_id } => {
            let req = client::GetAggregatedBoardRequest {
                aggregated_board_id: aggregate_id.clone(),
            };
            let chunks = client
                .get_aggregated_board(tonic::Request::new(req))
                .await
                .with_ctx(|| format!("get aggregated board {aggregate_id}"))?;

            let mut metadata: Option<client::AggregatedBoard> = None;
            let mut sources: Vec<client::SourceBoardRef> = Vec::new();
            let mut columns: Vec<client::AggregatedColumn> = Vec::new();
            let mut cards: Vec<client::Card> = Vec::new();

            for chunk in chunks {
                match chunk.payload {
                    Some(client::aggregated_board_chunk::Payload::Metadata(m)) => {
                        metadata = Some(m)
                    }
                    Some(client::aggregated_board_chunk::Payload::SourceBoard(s)) => {
                        sources.push(s)
                    }
                    Some(client::aggregated_board_chunk::Payload::Column(c)) => columns.push(c),
                    Some(client::aggregated_board_chunk::Payload::CardBatch(b)) => {
                        cards.extend(b.cards)
                    }
                    None => {}
                }
            }

            let output = serde_json::json!({
                "metadata": metadata.map(AggregatedBoardOut::from_proto),
                "sources": sources.into_iter().map(source_to_json).collect::<Vec<_>>(),
                "columns": columns.into_iter().map(column_to_json).collect::<Vec<_>>(),
                "cards": cards.into_iter().map(crate::kanban::cards::CardDetailOut::from_proto).collect::<Vec<_>>(),
            });
            render(&output, format)
        }
        AggregateAction::Create {
            name,
            description,
            icon,
            visibility,
        } => {
            let req = client::CreateAggregatedBoardRequest {
                name,
                description: description.unwrap_or_default(),
                icon: icon.unwrap_or_default(),
                source_board_ids: Vec::new(),
                idempotency_key: new_idempotency_key(),
                visibility: visibility
                    .map(|v| v.to_proto() as i32)
                    .unwrap_or(client::BoardVisibility::Private as i32),
            };
            let resp = client
                .create_aggregated_board(tonic::Request::new(req))
                .await
                .with_ctx(|| "create aggregated board".to_string())?;
            render(&AggregatedBoardOut::from_proto(resp), format)
        }
        AggregateAction::Update {
            aggregate_id,
            name,
            description,
            icon,
        } => {
            let mut board = client::AggregatedBoard {
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
            let req = client::UpdateAggregatedBoardRequest {
                aggregated_board_id: aggregate_id.clone(),
                aggregated_board: Some(board),
                update_mask: Some(prost_types::FieldMask { paths }),
            };
            let resp = client
                .update_aggregated_board(request_with_object_id(req, &aggregate_id)?)
                .await
                .with_ctx(|| format!("update aggregated board {aggregate_id}"))?;
            render(&AggregatedBoardOut::from_proto(resp), format)
        }
        AggregateAction::Delete { aggregate_id } => {
            let req = client::DeleteAggregatedBoardRequest {
                aggregated_board_id: aggregate_id.clone(),
            };
            client
                .delete_aggregated_board(request_with_object_id(req, &aggregate_id)?)
                .await
                .with_ctx(|| format!("delete aggregated board {aggregate_id}"))?;
            Ok(())
        }
        AggregateAction::Source { action } => match action {
            SourceAction::Add {
                aggregate_id,
                board_id,
                position,
            } => {
                let req = client::AddSourceBoardRequest {
                    aggregated_board_id: aggregate_id.clone(),
                    board_id: board_id.clone(),
                    position: position.unwrap_or_default(),
                };
                let resp = client
                    .add_source_board(request_with_object_id(req, &aggregate_id)?)
                    .await
                    .with_ctx(|| {
                        format!("add source board {board_id} to aggregate {aggregate_id}")
                    })?;
                render(&AggregatedBoardOut::from_proto(resp), format)
            }
            SourceAction::Remove {
                aggregate_id,
                board_id,
            } => {
                let req = client::RemoveSourceBoardRequest {
                    aggregated_board_id: aggregate_id.clone(),
                    board_id: board_id.clone(),
                };
                let resp = client
                    .remove_source_board(request_with_object_id(req, &aggregate_id)?)
                    .await
                    .with_ctx(|| {
                        format!("remove source board {board_id} from aggregate {aggregate_id}")
                    })?;
                render(&AggregatedBoardOut::from_proto(resp), format)
            }
            SourceAction::Move {
                aggregate_id,
                board_id,
                position,
            } => {
                let req = client::MoveSourceBoardRequest {
                    aggregated_board_id: aggregate_id.clone(),
                    board_id: board_id.clone(),
                    to_position: position,
                };
                let resp = client
                    .move_source_board(request_with_object_id(req, &aggregate_id)?)
                    .await
                    .with_ctx(|| {
                        format!("move source board {board_id} in aggregate {aggregate_id}")
                    })?;
                render(&AggregatedBoardOut::from_proto(resp), format)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputFormat;

    fn board(id: &str, name: &str) -> client::AggregatedBoard {
        client::AggregatedBoard {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            icon: String::new(),
            created_at: None,
            updated_at: None,
            visibility: client::BoardVisibility::Private as i32,
        }
    }

    #[tokio::test]
    async fn list_renders_aggregated_boards() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_list_aggregated_boards()
            .withf(|req| req.get_ref() == &client::ListAggregatedBoardsRequest {})
            .times(1)
            .returning(|_| {
                Ok(client::ListAggregatedBoardsResponse {
                    aggregated_boards: vec![board("agg_1", "Roadmap")],
                })
            });

        run(AggregateAction::List, OutputFormat::Json, &mut mock)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_renders_aggregated_board() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_get_aggregated_board()
            .withf(|req| req.get_ref().aggregated_board_id == "agg_1")
            .times(1)
            .returning(|_| {
                Ok(vec![
                    client::AggregatedBoardChunk {
                        payload: Some(client::aggregated_board_chunk::Payload::Metadata(board(
                            "agg_1", "Roadmap",
                        ))),
                    },
                    client::AggregatedBoardChunk {
                        payload: Some(client::aggregated_board_chunk::Payload::SourceBoard(
                            client::SourceBoardRef {
                                board_id: "board_1".into(),
                                project_id: "proj_1".into(),
                                name: "Source".into(),
                                icon: String::new(),
                                position: 0,
                            },
                        )),
                    },
                    client::AggregatedBoardChunk {
                        payload: Some(client::aggregated_board_chunk::Payload::Column(
                            client::AggregatedColumn {
                                id: "col_1".into(),
                                title: "Todo".into(),
                                accent: String::new(),
                                wip_limit: 0,
                                position: 0,
                                source_board_id: "board_1".into(),
                            },
                        )),
                    },
                    client::AggregatedBoardChunk {
                        payload: Some(client::aggregated_board_chunk::Payload::CardBatch(
                            client::AggregatedCardBatch {
                                cards: vec![client::Card {
                                    id: "card_1".into(),
                                    project_id: "proj_1".into(),
                                    board_id: "board_1".into(),
                                    column_id: "col_1".into(),
                                    r#ref: "PROJ-1".into(),
                                    title: "A card".into(),
                                    ..Default::default()
                                }],
                            },
                        )),
                    },
                ])
            });

        run(
            AggregateAction::Get {
                aggregate_id: "agg_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_renders_aggregated_board() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_create_aggregated_board()
            .withf(|req| {
                let r = req.get_ref();
                r.name == "Roadmap"
                    && r.description.is_empty()
                    && r.icon.is_empty()
                    && r.source_board_ids.is_empty()
                    && !r.idempotency_key.is_empty()
                    && r.visibility == client::BoardVisibility::Private as i32
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "Roadmap")));

        run(
            AggregateAction::Create {
                name: "Roadmap".into(),
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
    async fn update_renders_aggregated_board() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_update_aggregated_board()
            .withf(|req| {
                let r = req.get_ref();
                r.aggregated_board_id == "agg_1"
                    && r.aggregated_board
                        .as_ref()
                        .map(|b| b.id == "agg_1" && b.name == "Renamed")
                        .unwrap_or(false)
                    && r.update_mask
                        .as_ref()
                        .map(|m| m.paths == ["name"])
                        .unwrap_or(false)
                    && req.metadata()
                        .get("x-sunbeam-object-id")
                        .and_then(|v| v.to_str().ok())
                        == Some("agg_1")
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "Renamed")));

        run(
            AggregateAction::Update {
                aggregate_id: "agg_1".into(),
                name: Some("Renamed".into()),
                description: None,
                icon: None,
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_succeeds() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_delete_aggregated_board()
            .withf(|req| req.get_ref().aggregated_board_id == "agg_1")
            .times(1)
            .returning(|_| Ok(()));

        run(
            AggregateAction::Delete {
                aggregate_id: "agg_1".into(),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn source_add_renders_aggregated_board() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_add_source_board()
            .withf(|req| {
                let r = req.get_ref();
                r.aggregated_board_id == "agg_1" && r.board_id == "board_1" && r.position == 2
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "Roadmap")));

        run(
            AggregateAction::Source {
                action: SourceAction::Add {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                    position: Some(2),
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn source_remove_renders_aggregated_board() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_remove_source_board()
            .withf(|req| {
                let r = req.get_ref();
                r.aggregated_board_id == "agg_1" && r.board_id == "board_1"
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "Roadmap")));

        run(
            AggregateAction::Source {
                action: SourceAction::Remove {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn source_move_renders_aggregated_board() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_move_source_board()
            .withf(|req| {
                let r = req.get_ref();
                r.aggregated_board_id == "agg_1" && r.board_id == "board_1" && r.to_position == 3
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "Roadmap")));

        run(
            AggregateAction::Source {
                action: SourceAction::Move {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                    position: 3,
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_aggregated_boards_table_renders() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_list_aggregated_boards()
            .times(1)
            .returning(|_| {
                Ok(client::ListAggregatedBoardsResponse {
                    aggregated_boards: vec![board("agg_1", "Roadmap")],
                })
            });

        run(AggregateAction::List, OutputFormat::Table, &mut mock)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_aggregated_board_with_all_options_renders() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_create_aggregated_board()
            .withf(|req| {
                let r = req.get_ref();
                r.name == "N"
                    && r.description == "D"
                    && r.icon == "I"
                    && r.visibility == client::BoardVisibility::Public as i32
                    && !r.idempotency_key.is_empty()
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "N")));

        run(
            AggregateAction::Create {
                name: "N".into(),
                description: Some("D".into()),
                icon: Some("I".into()),
                visibility: Some(VisibilityArg::Public),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_aggregated_board_with_all_options_renders() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_update_aggregated_board()
            .withf(|req| {
                let r = req.get_ref();
                let paths = r.update_mask.as_ref().map(|m| m.paths.clone());
                r.aggregated_board_id == "agg_1"
                    && paths == Some(vec!["name".into(), "description".into(), "icon".into()])
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "N")));

        run(
            AggregateAction::Update {
                aggregate_id: "agg_1".into(),
                name: Some("N".into()),
                description: Some("D".into()),
                icon: Some("I".into()),
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn source_add_with_default_position_renders() {
        let mut mock = MockAggregatedBoardService::new();
        mock.expect_add_source_board()
            .withf(|req| {
                let r = req.get_ref();
                r.aggregated_board_id == "agg_1" && r.board_id == "board_1" && r.position == 0
            })
            .times(1)
            .returning(|_| Ok(board("agg_1", "Roadmap")));

        run(
            AggregateAction::Source {
                action: SourceAction::Add {
                    aggregate_id: "agg_1".into(),
                    board_id: "board_1".into(),
                    position: None,
                },
            },
            OutputFormat::Json,
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[test]
    fn fmt_ts_handles_missing_and_invalid() {
        assert!(fmt_ts(None).is_empty());
        assert!(
            fmt_ts(Some(&prost_types::Timestamp {
                seconds: i64::MAX,
                nanos: 0,
            }))
            .is_empty()
        );
    }

    #[test]
    fn visibility_arg_to_proto_covers_all() {
        assert_eq!(
            VisibilityArg::Private.to_proto() as i32,
            client::BoardVisibility::Private as i32
        );
        assert_eq!(
            VisibilityArg::Internal.to_proto() as i32,
            client::BoardVisibility::Internal as i32
        );
        assert_eq!(
            VisibilityArg::Public.to_proto() as i32,
            client::BoardVisibility::Public as i32
        );
    }

    #[tokio::test]
    async fn build_client_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build_client(&logger, ":::bad", "token").await.unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }
}
