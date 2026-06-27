//! Kanban realtime subscription commands.

use crate::error::{Result, ResultExt};
use crate::kanban::client::{self, AuthChannel, BoardServiceClient, ProjectServiceClient};
use crate::kanban::require_token;
use crate::logger::Logger;
use crate::wfectl::struct_util::prost_struct_to_json;
use async_trait::async_trait;
use clap::Subcommand;
use futures::stream::{Stream, StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::pin::Pin;

/// Subscription actions.
#[derive(Debug, Subcommand)]
pub enum SubscribeAction {
    /// Subscribe to board events.
    Board {
        /// Board ID or name.
        board_id: String,
    },
    /// Subscribe to project events.
    Project {
        /// Project ID or name.
        project_id: String,
    },
}

/// Stream of realtime board/project events.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<client::BoardEventEnvelope>> + Send>>;

/// Trait abstracting the Kanban subscription services for testability.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait SubscriptionService {
    /// Subscribe to board-level events.
    async fn subscribe_board(
        &mut self,
        req: tonic::Request<client::SubscribeBoardRequest>,
    ) -> Result<EventStream>;
    /// Subscribe to project-level events.
    async fn subscribe_project(
        &mut self,
        req: tonic::Request<client::SubscribeProjectRequest>,
    ) -> Result<EventStream>;
}

/// Wrapper around the generated Tonic board and project clients.
#[derive(Debug)]
pub struct SubscriptionServiceClientWrapper {
    board_client: BoardServiceClient<AuthChannel>,
    project_client: ProjectServiceClient<AuthChannel>,
}

impl SubscriptionServiceClientWrapper {
    /// Build a wrapper from authenticated board and project clients.
    pub fn new(
        board_client: BoardServiceClient<AuthChannel>,
        project_client: ProjectServiceClient<AuthChannel>,
    ) -> Self {
        Self {
            board_client,
            project_client,
        }
    }
}

#[async_trait]
impl SubscriptionService for SubscriptionServiceClientWrapper {
    async fn subscribe_board(
        &mut self,
        req: tonic::Request<client::SubscribeBoardRequest>,
    ) -> Result<EventStream> {
        let stream = self.board_client.subscribe_board(req).await?.into_inner();
        Ok(Box::pin(stream.map_err(|e| e.into())))
    }

    async fn subscribe_project(
        &mut self,
        req: tonic::Request<client::SubscribeProjectRequest>,
    ) -> Result<EventStream> {
        let stream = self
            .project_client
            .subscribe_project(req)
            .await?
            .into_inner();
        Ok(Box::pin(stream.map_err(|e| e.into())))
    }
}

/// Build a subscription-service client wrapper for the given server and token.
pub async fn build_client(
    logger: &Logger,
    server: &str,
    token: &str,
) -> Result<SubscriptionServiceClientWrapper> {
    let channel = client::build(logger, server, token).await?;
    let board_client = BoardServiceClient::new(channel.clone());
    let project_client = ProjectServiceClient::new(channel);
    Ok(SubscriptionServiceClientWrapper::new(
        board_client,
        project_client,
    ))
}

fn ts_to_json(ts: &prost_types::Timestamp) -> Value {
    chrono::DateTime::from_timestamp(ts.seconds, ts.nanos.max(0) as u32)
        .map(|dt| Value::String(dt.to_rfc3339()))
        .unwrap_or(Value::Null)
}

fn opt_ts(ts: &Option<prost_types::Timestamp>) -> Value {
    ts.as_ref().map_or(Value::Null, ts_to_json)
}

fn priority_str(value: i32) -> &'static str {
    match value {
        1 => "low",
        2 => "medium",
        3 => "high",
        4 => "urgent",
        _ => "unspecified",
    }
}

fn urgency_str(value: i32) -> &'static str {
    match value {
        1 => "low",
        2 => "medium",
        3 => "high",
        4 => "critical",
        _ => "unspecified",
    }
}

fn label_to_json(label: &client::Label) -> Value {
    json!({
        "id": label.id,
        "project_id": label.project_id,
        "name": label.name,
        "style": label.style,
    })
}

fn assignee_to_json(assignee: &client::Assignee) -> Value {
    json!({
        "subject": assignee.subject,
        "display_name": assignee.display_name,
        "avatar_url": assignee.avatar_url,
    })
}

fn checklist_item_to_json(item: &client::ChecklistItem) -> Value {
    json!({
        "id": item.id,
        "text": item.text,
        "done": item.done,
        "position": item.position,
    })
}

fn github_link_to_json(link: &client::GitHubLink) -> Value {
    json!({
        "id": link.id,
        "repo": link.repo,
        "number": link.number,
        "state": link.state,
        "merged": link.merged,
        "last_synced_at": opt_ts(&link.last_synced_at),
    })
}

fn card_to_json(card: &client::Card) -> Value {
    json!({
        "id": card.id,
        "project_id": card.project_id,
        "board_id": card.board_id,
        "column_id": card.column_id,
        "ref": card.r#ref,
        "title": card.title,
        "description": card.description,
        "priority": priority_str(card.priority),
        "due": opt_ts(&card.due),
        "completed_at": opt_ts(&card.completed_at),
        "blocked": card.blocked,
        "cover": card.cover,
        "milestone_id": card.milestone_id,
        "position": card.position,
        "labels": card.labels.iter().map(label_to_json).collect::<Vec<_>>(),
        "assignees": card.assignees.iter().map(assignee_to_json).collect::<Vec<_>>(),
        "checklist": card.checklist.iter().map(checklist_item_to_json).collect::<Vec<_>>(),
        "github_links": card.github_links.iter().map(github_link_to_json).collect::<Vec<_>>(),
        "comments_count": card.comments_count,
        "attachments_count": card.attachments_count,
        "revision": card.revision,
        "created_at": opt_ts(&card.created_at),
        "updated_at": opt_ts(&card.updated_at),
        "urgency": urgency_str(card.urgency),
        "depends_on_card_ids": card.depends_on_card_ids,
        "dependent_card_ids": card.dependent_card_ids,
    })
}

fn event_column_to_json(col: &client::EventColumn) -> Value {
    json!({
        "id": col.id,
        "board_id": col.board_id,
        "title": col.title,
        "accent": col.accent,
        "wip_limit": col.wip_limit,
        "position": col.position,
    })
}

fn event_board_to_json(board: &client::EventBoard) -> Value {
    json!({
        "id": board.id,
        "project_id": board.project_id,
        "name": board.name,
        "description": board.description,
        "icon": board.icon,
    })
}

fn payload_to_json(payload: &client::board_event_envelope::Payload) -> Value {
    use client::board_event_envelope::Payload::*;
    match payload {
        CardCreated(e) => json!({
            "type": "CardCreated",
            "card": e.card.as_ref().map(card_to_json),
            "column_id": e.column_id,
            "position": e.position,
            "idempotency_key": e.idempotency_key,
        }),
        CardUpdated(e) => json!({
            "type": "CardUpdated",
            "card_id": e.card_id,
            "prev_revision": e.prev_revision,
            "new_revision": e.new_revision,
            "patch": e.patch.as_ref().map_or(Value::Null, prost_struct_to_json),
            "idempotency_key": e.idempotency_key,
        }),
        CardMoved(e) => json!({
            "type": "CardMoved",
            "card_id": e.card_id,
            "from_column": e.from_column,
            "to_column": e.to_column,
            "to_position": e.to_position,
            "prev_revision": e.prev_revision,
            "new_revision": e.new_revision,
            "idempotency_key": e.idempotency_key,
        }),
        CardDeleted(e) => json!({
            "type": "CardDeleted",
            "card_id": e.card_id,
            "prev_revision": e.prev_revision,
            "idempotency_key": e.idempotency_key,
        }),
        ColumnAdded(e) => json!({
            "type": "ColumnAdded",
            "column": e.column.as_ref().map(event_column_to_json),
            "position": e.position,
        }),
        ColumnRenamed(e) => json!({
            "type": "ColumnRenamed",
            "column_id": e.column_id,
            "new_title": e.new_title,
        }),
        ColumnRemoved(e) => json!({
            "type": "ColumnRemoved",
            "column_id": e.column_id,
            "move_cards_to_column": e.move_cards_to_column,
        }),
        ColumnUpdated(e) => json!({
            "type": "ColumnUpdated",
            "column": e.column.as_ref().map(event_column_to_json),
        }),
        ColumnsReordered(e) => json!({
            "type": "ColumnsReordered",
            "columns": e.columns.iter().map(event_column_to_json).collect::<Vec<_>>(),
        }),
        BoardRenamed(e) => json!({
            "type": "BoardRenamed",
            "new_name": e.new_name,
        }),
        BoardUpdated(e) => json!({
            "type": "BoardUpdated",
            "board": e.board.as_ref().map(event_board_to_json),
        }),
        MembershipChanged(e) => json!({
            "type": "MembershipChanged",
            "subject": e.subject,
            "relation": e.relation,
            "granted": e.granted,
        }),
        MemberAdded(e) => json!({
            "type": "MemberAdded",
            "project_id": e.project_id,
            "subject": e.subject,
            "relation": e.relation,
            "display_name": e.display_name,
            "email": e.email,
        }),
        MemberRemoved(e) => json!({
            "type": "MemberRemoved",
            "project_id": e.project_id,
            "subject": e.subject,
        }),
        MemberRoleChanged(e) => json!({
            "type": "MemberRoleChanged",
            "project_id": e.project_id,
            "subject": e.subject,
            "old_relation": e.old_relation,
            "new_relation": e.new_relation,
        }),
        BoardCreated(e) => json!({
            "type": "BoardCreated",
            "project_id": e.project_id,
            "board_id": e.board_id,
            "name": e.name,
        }),
        BoardDeleted(e) => json!({
            "type": "BoardDeleted",
            "project_id": e.project_id,
            "board_id": e.board_id,
        }),
        ProjectUpdated(e) => json!({
            "type": "ProjectUpdated",
            "project_id": e.project_id,
            "name": e.name,
            "prefix": e.prefix,
            "description": e.description,
        }),
        ChecklistUpdated(e) => json!({
            "type": "ChecklistUpdated",
            "card_id": e.card_id,
            "done": e.done,
            "total": e.total,
        }),
        CommentAdded(e) => json!({
            "type": "CommentAdded",
            "card_id": e.card_id,
            "comment_id": e.comment_id,
            "author_sub": e.author_sub,
        }),
        CommentEdited(e) => json!({
            "type": "CommentEdited",
            "card_id": e.card_id,
            "comment_id": e.comment_id,
        }),
        CommentDeleted(e) => json!({
            "type": "CommentDeleted",
            "card_id": e.card_id,
            "comment_id": e.comment_id,
        }),
        AttachmentAdded(e) => json!({
            "type": "AttachmentAdded",
            "card_id": e.card_id,
            "attachment_id": e.attachment_id,
            "filename": e.filename,
        }),
        AttachmentDeleted(e) => json!({
            "type": "AttachmentDeleted",
            "card_id": e.card_id,
            "attachment_id": e.attachment_id,
        }),
        GithubLinkAdded(e) => json!({
            "type": "GitHubLinkAdded",
            "card_id": e.card_id,
            "link_id": e.link_id,
        }),
        GithubLinkRefreshed(e) => json!({
            "type": "GitHubLinkRefreshed",
            "card_id": e.card_id,
            "link_id": e.link_id,
            "new_state": e.new_state,
        }),
        AggregatedBoardCreated(e) => json!({
            "type": "AggregatedBoardCreated",
            "aggregated_board_id": e.aggregated_board_id,
        }),
        AggregatedBoardUpdated(e) => json!({
            "type": "AggregatedBoardUpdated",
            "aggregated_board_id": e.aggregated_board_id,
        }),
        AggregatedBoardDeleted(e) => json!({
            "type": "AggregatedBoardDeleted",
            "aggregated_board_id": e.aggregated_board_id,
        }),
        SourceBoardAdded(e) => json!({
            "type": "SourceBoardAdded",
            "aggregated_board_id": e.aggregated_board_id,
            "board_id": e.board_id,
        }),
        SourceBoardRemoved(e) => json!({
            "type": "SourceBoardRemoved",
            "aggregated_board_id": e.aggregated_board_id,
            "board_id": e.board_id,
        }),
        Heartbeat(e) => json!({
            "type": "Heartbeat",
            "server_time_ms": e.server_time_ms,
        }),
        Cutover(e) => json!({
            "type": "Cutover",
            "last_replay_nats_seq": e.last_replay_nats_seq,
        }),
    }
}

fn envelope_to_json(envelope: &client::BoardEventEnvelope) -> Value {
    json!({
        "board_id": envelope.board_id,
        "event_id": envelope.event_id,
        "nats_seq": envelope.nats_seq,
        "board_revision": envelope.board_revision,
        "emitted_at": opt_ts(&envelope.emitted_at),
        "emitter_pod_id": envelope.emitter_pod_id,
        "actor_subject": envelope.actor_subject,
        "payload": envelope.payload.as_ref().map_or(Value::Null, payload_to_json),
    })
}

fn print_envelope(envelope: &client::BoardEventEnvelope) -> Result<()> {
    let value = envelope_to_json(envelope);
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// Run a subscription command using the provided service client.
pub async fn run_with_client(
    cmd: SubscribeAction,
    client: &mut dyn SubscriptionService,
) -> Result<()> {
    match cmd {
        SubscribeAction::Board { board_id } => {
            let req = tonic::Request::new(client::SubscribeBoardRequest {
                board_id,
                since_seq: 0,
            });
            let mut stream = client
                .subscribe_board(req)
                .await
                .with_ctx(|| "kanban subscribe board failed".to_string())?;

            while let Some(envelope) = stream.next().await {
                let envelope =
                    envelope.with_ctx(|| "kanban subscribe board stream failed".to_string())?;
                print_envelope(&envelope)?;
            }
        }
        SubscribeAction::Project { project_id } => {
            let req = client::request_with_object_id(
                client::SubscribeProjectRequest {
                    project_id: project_id.clone(),
                    since_seq: 0,
                },
                &project_id,
            )?;
            let mut stream = client
                .subscribe_project(req)
                .await
                .with_ctx(|| "kanban subscribe project failed".to_string())?;

            while let Some(envelope) = stream.next().await {
                let envelope =
                    envelope.with_ctx(|| "kanban subscribe project stream failed".to_string())?;
                print_envelope(&envelope)?;
            }
        }
    }

    Ok(())
}

/// Run a subscription command.
pub async fn run(logger: &Logger, cmd: SubscribeAction, server: &str) -> Result<()> {
    let token = require_token().await?;
    let mut client = build_client(logger, server, &token).await?;
    run_with_client(cmd, &mut client).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::StreamExt;

    fn heartbeat_envelope(board_id: &str) -> client::BoardEventEnvelope {
        client::BoardEventEnvelope {
            board_id: board_id.into(),
            event_id: "evt_1".into(),
            nats_seq: 1,
            board_revision: 1,
            emitted_at: Some(prost_types::Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            emitter_pod_id: "pod_1".into(),
            actor_subject: "user:1".into(),
            payload: Some(client::board_event_envelope::Payload::Heartbeat(
                client::Heartbeat {
                    server_time_ms: 1_700_000_000_000,
                },
            )),
        }
    }

    #[tokio::test]
    async fn subscribe_board_renders_events() {
        let mut mock = MockSubscriptionService::new();
        mock.expect_subscribe_board()
            .withf(|req| req.get_ref().board_id == "board_123" && req.get_ref().since_seq == 0)
            .times(1)
            .returning(|_| {
                Ok(futures::stream::iter(vec![Ok(heartbeat_envelope("board_123"))]).boxed())
            });

        run_with_client(
            SubscribeAction::Board {
                board_id: "board_123".into(),
            },
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn subscribe_project_renders_events() {
        let mut mock = MockSubscriptionService::new();
        mock.expect_subscribe_project()
            .withf(|req| {
                req.get_ref().project_id == "proj_123"
                    && req.get_ref().since_seq == 0
                    && req
                        .metadata()
                        .get("x-sunbeam-object-id")
                        .and_then(|v| v.to_str().ok())
                        == Some("proj_123")
            })
            .times(1)
            .returning(|_| Ok(futures::stream::iter(vec![Ok(heartbeat_envelope(""))]).boxed()));

        run_with_client(
            SubscribeAction::Project {
                project_id: "proj_123".into(),
            },
            &mut mock,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn subscribe_board_stream_error_propagates() {
        let mut mock = MockSubscriptionService::new();
        mock.expect_subscribe_board().times(1).returning(|_| {
            Ok(
                futures::stream::iter(vec![Err(crate::error::SunbeamError::Other(
                    "stream boom".into(),
                ))])
                .boxed(),
            )
        });

        let err = run_with_client(
            SubscribeAction::Board {
                board_id: "board_123".into(),
            },
            &mut mock,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("stream boom"));
    }

    #[test]
    fn helpers_handle_edge_cases() {
        let valid = prost_types::Timestamp {
            seconds: 1,
            nanos: 500_000_000,
        };
        assert!(ts_to_json(&valid).as_str().unwrap().contains("T"));

        let negative_nanos = prost_types::Timestamp {
            seconds: 1,
            nanos: -1,
        };
        assert!(ts_to_json(&negative_nanos).as_str().unwrap().contains("T"));

        assert_eq!(opt_ts(&None), Value::Null);

        assert_eq!(priority_str(1), "low");
        assert_eq!(priority_str(2), "medium");
        assert_eq!(priority_str(3), "high");
        assert_eq!(priority_str(4), "urgent");
        assert_eq!(priority_str(99), "unspecified");

        assert_eq!(urgency_str(1), "low");
        assert_eq!(urgency_str(2), "medium");
        assert_eq!(urgency_str(3), "high");
        assert_eq!(urgency_str(4), "critical");
        assert_eq!(urgency_str(99), "unspecified");
    }

    #[test]
    fn card_and_helpers_json_cover_all_fields() {
        let label = client::Label {
            id: "l1".into(),
            project_id: "p1".into(),
            name: "bug".into(),
            style: "red".into(),
        };
        let assignee = client::Assignee {
            subject: "sub".into(),
            display_name: "Ada".into(),
            avatar_url: "http://a".into(),
        };
        let item = client::ChecklistItem {
            id: "i1".into(),
            text: "do it".into(),
            done: true,
            position: 1,
        };
        let link = client::GitHubLink {
            id: "g1".into(),
            repo: "r".into(),
            number: 1,
            state: "open".into(),
            merged: false,
            last_synced_at: None,
        };
        let card = client::Card {
            id: "c1".into(),
            project_id: "p1".into(),
            board_id: "b1".into(),
            column_id: "col1".into(),
            r#ref: "REF-1".into(),
            title: "t".into(),
            description: "d".into(),
            priority: 2,
            due: None,
            completed_at: None,
            blocked: false,
            cover: "cov".into(),
            milestone_id: "m1".into(),
            position: 1,
            labels: vec![label],
            assignees: vec![assignee],
            checklist: vec![item],
            github_links: vec![link],
            comments_count: 3,
            attachments_count: 2,
            revision: 5,
            created_at: None,
            updated_at: None,
            urgency: 3,
            depends_on_card_ids: vec!["c2".into()],
            dependent_card_ids: vec!["c3".into()],
        };
        let value = card_to_json(&card);
        assert_eq!(value["priority"], "medium");
        assert_eq!(value["urgency"], "high");
        assert!(value["labels"].as_array().unwrap().len() == 1);
    }

    fn payload_envelope(
        payload: client::board_event_envelope::Payload,
    ) -> client::BoardEventEnvelope {
        client::BoardEventEnvelope {
            board_id: "b".into(),
            event_id: "e".into(),
            nats_seq: 1,
            board_revision: 1,
            emitted_at: Some(prost_types::Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            emitter_pod_id: "pod".into(),
            actor_subject: "user:1".into(),
            payload: Some(payload),
        }
    }

    #[tokio::test]
    async fn build_client_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = build_client(&logger, ":::bad", "token").await.unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }

    #[tokio::test]
    async fn all_payload_variants_renders() {
        use client::board_event_envelope::Payload::*;
        let column = client::EventColumn {
            id: "col".into(),
            board_id: "b".into(),
            title: "Col".into(),
            accent: "blue".into(),
            wip_limit: 5,
            position: 1,
        };
        let board = client::EventBoard {
            id: "b".into(),
            project_id: "p".into(),
            name: "B".into(),
            description: "d".into(),
            icon: "i".into(),
        };
        let card = client::Card {
            id: "c1".into(),
            project_id: "p".into(),
            board_id: "b".into(),
            column_id: "col".into(),
            r#ref: "REF-1".into(),
            title: "t".into(),
            description: "d".into(),
            priority: 1,
            due: None,
            completed_at: None,
            blocked: false,
            cover: "".into(),
            milestone_id: "".into(),
            position: 1,
            labels: vec![],
            assignees: vec![],
            checklist: vec![],
            github_links: vec![],
            comments_count: 0,
            attachments_count: 0,
            revision: 1,
            created_at: None,
            updated_at: None,
            urgency: 1,
            depends_on_card_ids: vec![],
            dependent_card_ids: vec![],
        };

        let events: Vec<client::BoardEventEnvelope> = vec![
            payload_envelope(CardCreated(client::CardCreated {
                card: Some(card.clone()),
                column_id: "col".into(),
                position: 1,
                idempotency_key: "ik".into(),
            })),
            payload_envelope(CardUpdated(client::CardUpdated {
                card_id: "c1".into(),
                prev_revision: 1,
                new_revision: 2,
                patch: None,
                idempotency_key: "ik".into(),
            })),
            payload_envelope(CardMoved(client::CardMoved {
                card_id: "c1".into(),
                from_column: "c1".into(),
                to_column: "c2".into(),
                to_position: 2,
                prev_revision: 1,
                new_revision: 2,
                idempotency_key: "ik".into(),
            })),
            payload_envelope(CardDeleted(client::CardDeleted {
                card_id: "c1".into(),
                prev_revision: 1,
                idempotency_key: "ik".into(),
            })),
            payload_envelope(ColumnAdded(client::ColumnAdded {
                column: Some(column.clone()),
                position: 1,
            })),
            payload_envelope(ColumnRenamed(client::ColumnRenamed {
                column_id: "col".into(),
                new_title: "New".into(),
            })),
            payload_envelope(ColumnRemoved(client::ColumnRemoved {
                column_id: "col".into(),
                move_cards_to_column: "c2".into(),
            })),
            payload_envelope(ColumnUpdated(client::ColumnUpdated {
                column: Some(column.clone()),
            })),
            payload_envelope(ColumnsReordered(client::ColumnsReordered {
                columns: vec![column.clone()],
            })),
            payload_envelope(BoardRenamed(client::BoardRenamed {
                new_name: "N".into(),
            })),
            payload_envelope(BoardUpdated(client::BoardUpdated {
                board: Some(board.clone()),
            })),
            payload_envelope(MembershipChanged(client::MembershipChanged {
                subject: "s".into(),
                relation: "r".into(),
                granted: true,
            })),
            payload_envelope(MemberAdded(client::MemberAdded {
                project_id: "p".into(),
                subject: "s".into(),
                relation: "r".into(),
                display_name: "D".into(),
                email: "e@x".into(),
            })),
            payload_envelope(MemberRemoved(client::MemberRemoved {
                project_id: "p".into(),
                subject: "s".into(),
            })),
            payload_envelope(MemberRoleChanged(client::MemberRoleChanged {
                project_id: "p".into(),
                subject: "s".into(),
                old_relation: "r".into(),
                new_relation: "a".into(),
            })),
            payload_envelope(BoardCreated(client::BoardCreated {
                project_id: "p".into(),
                board_id: "b".into(),
                name: "B".into(),
            })),
            payload_envelope(BoardDeleted(client::BoardDeleted {
                project_id: "p".into(),
                board_id: "b".into(),
            })),
            payload_envelope(ProjectUpdated(client::ProjectUpdated {
                project_id: "p".into(),
                name: "P".into(),
                prefix: "PR".into(),
                description: "d".into(),
            })),
            payload_envelope(ChecklistUpdated(client::ChecklistUpdated {
                card_id: "c1".into(),
                done: 1,
                total: 2,
            })),
            payload_envelope(CommentAdded(client::CommentAdded {
                card_id: "c1".into(),
                comment_id: "cm1".into(),
                author_sub: "a".into(),
            })),
            payload_envelope(CommentEdited(client::CommentEdited {
                card_id: "c1".into(),
                comment_id: "cm1".into(),
            })),
            payload_envelope(CommentDeleted(client::CommentDeleted {
                card_id: "c1".into(),
                comment_id: "cm1".into(),
            })),
            payload_envelope(AttachmentAdded(client::AttachmentAdded {
                card_id: "c1".into(),
                attachment_id: "a1".into(),
                filename: "f".into(),
            })),
            payload_envelope(AttachmentDeleted(client::AttachmentDeleted {
                card_id: "c1".into(),
                attachment_id: "a1".into(),
            })),
            payload_envelope(GithubLinkAdded(client::GitHubLinkAdded {
                card_id: "c1".into(),
                link_id: "l1".into(),
            })),
            payload_envelope(GithubLinkRefreshed(client::GitHubLinkRefreshed {
                card_id: "c1".into(),
                link_id: "l1".into(),
                new_state: "closed".into(),
            })),
            payload_envelope(AggregatedBoardCreated(client::AggregatedBoardCreated {
                aggregated_board_id: "ab".into(),
            })),
            payload_envelope(AggregatedBoardUpdated(client::AggregatedBoardUpdated {
                aggregated_board_id: "ab".into(),
            })),
            payload_envelope(AggregatedBoardDeleted(client::AggregatedBoardDeleted {
                aggregated_board_id: "ab".into(),
            })),
            payload_envelope(SourceBoardAdded(client::SourceBoardAdded {
                aggregated_board_id: "ab".into(),
                board_id: "b".into(),
            })),
            payload_envelope(SourceBoardRemoved(client::SourceBoardRemoved {
                aggregated_board_id: "ab".into(),
                board_id: "b".into(),
            })),
            payload_envelope(Cutover(client::Cutover {
                last_replay_nats_seq: 42,
            })),
        ];

        let mut mock = MockSubscriptionService::new();
        mock.expect_subscribe_board().times(1).returning(move |_| {
            Ok(futures::stream::iter(events.clone().into_iter().map(Ok)).boxed())
        });

        run_with_client(
            SubscribeAction::Board {
                board_id: "b".into(),
            },
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
            SubscribeAction::Board {
                board_id: "b".into(),
            },
            ":::not-a-url",
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("login")
                || err.to_string().contains("invalid kanban server URL"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn wrapper_new_constructs() {
        let channel = tonic::transport::Endpoint::from_static("http://[::1]:1").connect_lazy();
        let auth = crate::kanban::client::BearerAuth::new("").unwrap();
        let auth_channel = tonic::service::interceptor::InterceptedService::new(channel, auth);
        let board_client = BoardServiceClient::new(auth_channel.clone());
        let project_client = ProjectServiceClient::new(auth_channel);
        let _wrapper = SubscriptionServiceClientWrapper::new(board_client, project_client);
    }
}
