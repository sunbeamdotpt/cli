//! Kanban realtime subscription commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::v1;
use sdk::kanban::v1::__buffa::oneof::board_event_envelope::Payload;
use serde::Serialize;
use serde_json::{Value, json};

use super::{fmt_ts, object_id_options};

/// Subscription actions.
#[derive(Debug, Clone, Subcommand)]
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

/// Resolve a kanban OIDC subject to an email address through the sso-gateway
/// IdentityService.
///
/// Unresolvable or malformed subjects return `None` so that event
/// streaming keeps going even when the gateway is temporarily unreachable.
async fn resolve_subject_email(subject: &str) -> Option<String> {
    match crate::auth::resolve_email_for_subject(subject).await {
        Ok(email) if !email.is_empty() => Some(email),
        _ => None,
    }
}

/// Insert a `"type"` discriminator into a serialized event payload.
fn typed(ty: &str, value: Value) -> Value {
    let mut value = value;
    if let Value::Object(ref mut map) = value {
        map.insert("type".to_string(), Value::String(ty.to_string()));
    }
    value
}

/// Serialize an event payload struct to JSON (RFC 3339 timestamps, natural
/// `google.protobuf.Struct` encoding via the buffa serde impls).
fn to_value<T: Serialize>(event: &T) -> Value {
    serde_json::to_value(event).unwrap_or(Value::Null)
}

/// Attach a resolved `"email"` field for `subject` to a payload object.
async fn with_email(mut value: Value, subject: &str) -> Value {
    value["email"] = json!(resolve_subject_email(subject).await);
    value
}

async fn payload_to_json(payload: &Payload) -> Value {
    use Payload::*;
    match payload {
        CardCreated(e) => {
            let mut value = typed("CardCreated", to_value(e.as_ref()));
            if let Some(card) = e.card.as_option() {
                // Prefer the server-populated assignee email (kanban
                // v2026.07.12+); older servers leave it empty, so fall back
                // to the identity lookup.
                let emails = futures::future::join_all(card.assignees.iter().map(|a| async {
                    if a.email.is_empty() {
                        resolve_subject_email(&a.subject).await
                    } else {
                        Some(a.email.clone())
                    }
                }))
                .await;
                if let Some(assignees) = value
                    .pointer_mut("/card/assignees")
                    .and_then(Value::as_array_mut)
                {
                    for (item, email) in assignees.iter_mut().zip(emails) {
                        item["email"] = json!(email);
                    }
                }
            }
            value
        }
        CardUpdated(e) => typed("CardUpdated", to_value(e.as_ref())),
        CardMoved(e) => typed("CardMoved", to_value(e.as_ref())),
        CardTransferred(e) => typed("CardTransferred", to_value(e.as_ref())),
        CardDeleted(e) => typed("CardDeleted", to_value(e.as_ref())),
        ColumnAdded(e) => typed("ColumnAdded", to_value(e.as_ref())),
        ColumnRenamed(e) => typed("ColumnRenamed", to_value(e.as_ref())),
        ColumnRemoved(e) => typed("ColumnRemoved", to_value(e.as_ref())),
        ColumnUpdated(e) => typed("ColumnUpdated", to_value(e.as_ref())),
        ColumnsReordered(e) => typed("ColumnsReordered", to_value(e.as_ref())),
        BoardRenamed(e) => typed("BoardRenamed", to_value(e.as_ref())),
        BoardUpdated(e) => typed("BoardUpdated", to_value(e.as_ref())),
        MembershipChanged(e) => {
            with_email(typed("MembershipChanged", to_value(e.as_ref())), &e.subject).await
        }
        MemberAdded(e) => typed("MemberAdded", to_value(e.as_ref())),
        MemberRemoved(e) => {
            with_email(typed("MemberRemoved", to_value(e.as_ref())), &e.subject).await
        }
        MemberRoleChanged(e) => {
            with_email(typed("MemberRoleChanged", to_value(e.as_ref())), &e.subject).await
        }
        BoardCreated(e) => typed("BoardCreated", to_value(e.as_ref())),
        BoardDeleted(e) => typed("BoardDeleted", to_value(e.as_ref())),
        ProjectUpdated(e) => typed("ProjectUpdated", to_value(e.as_ref())),
        ChecklistUpdated(e) => typed("ChecklistUpdated", to_value(e.as_ref())),
        CommentAdded(e) => {
            let mut value = typed("CommentAdded", to_value(e.as_ref()));
            value["author_email"] = json!(resolve_subject_email(&e.author_sub).await);
            value
        }
        CommentEdited(e) => typed("CommentEdited", to_value(e.as_ref())),
        CommentDeleted(e) => typed("CommentDeleted", to_value(e.as_ref())),
        AttachmentAdded(e) => typed("AttachmentAdded", to_value(e.as_ref())),
        AttachmentDeleted(e) => typed("AttachmentDeleted", to_value(e.as_ref())),
        GithubLinkAdded(e) => typed("GitHubLinkAdded", to_value(e.as_ref())),
        GithubLinkRefreshed(e) => typed("GitHubLinkRefreshed", to_value(e.as_ref())),
        AggregatedBoardCreated(e) => typed("AggregatedBoardCreated", to_value(e.as_ref())),
        AggregatedBoardUpdated(e) => typed("AggregatedBoardUpdated", to_value(e.as_ref())),
        AggregatedBoardDeleted(e) => typed("AggregatedBoardDeleted", to_value(e.as_ref())),
        SourceBoardAdded(e) => typed("SourceBoardAdded", to_value(e.as_ref())),
        SourceBoardRemoved(e) => typed("SourceBoardRemoved", to_value(e.as_ref())),
        Heartbeat(e) => typed("Heartbeat", to_value(e.as_ref())),
        Cutover(e) => typed("Cutover", to_value(e.as_ref())),
    }
}

async fn envelope_to_json(envelope: &v1::BoardEventEnvelope) -> Value {
    let actor_email = resolve_subject_email(&envelope.actor_subject).await;
    let payload = match envelope.payload.as_ref() {
        Some(p) => payload_to_json(p).await,
        None => Value::Null,
    };
    json!({
        "board_id": envelope.board_id,
        "event_id": envelope.event_id,
        "nats_seq": envelope.nats_seq,
        "board_revision": envelope.board_revision,
        "emitted_at": fmt_ts(&envelope.emitted_at),
        "emitter_pod_id": envelope.emitter_pod_id,
        "actor_subject": envelope.actor_subject,
        "actor_email": actor_email,
        "payload": payload,
    })
}

async fn print_envelope(envelope: &v1::BoardEventEnvelope) -> Result<()> {
    let value = envelope_to_json(envelope).await;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// Run a subscription command, streaming events until the server closes the
/// stream or an error occurs.
pub(crate) async fn run(cmd: SubscribeAction, client: &KanbanClient) -> Result<()> {
    match cmd {
        SubscribeAction::Board { board_id } => {
            let mut stream = client
                .boards()
                .subscribe_board(v1::SubscribeBoardRequest {
                    board_id,
                    since_seq: 0,
                    ..Default::default()
                })
                .await?;
            while let Some(resp) = stream.message().await? {
                if let Some(envelope) = resp.to_owned_message().envelope.into_option() {
                    print_envelope(&envelope).await?;
                }
            }
        }
        SubscribeAction::Project { project_id } => {
            let mut stream = client
                .projects()
                .subscribe_project_with_options(
                    v1::SubscribeProjectRequest {
                        project_id: project_id.clone(),
                        since_seq: 0,
                        ..Default::default()
                    },
                    object_id_options(&project_id),
                )
                .await?;
            while let Some(resp) = stream.message().await? {
                if let Some(envelope) = resp.to_owned_message().envelope.into_option() {
                    print_envelope(&envelope).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kanban::testutil;
    use sdk::kanban::prelude::buffa::Message;
    use sdk::kanban::prelude::buffa_types;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn envelope(payload: Option<Payload>) -> v1::BoardEventEnvelope {
        v1::BoardEventEnvelope {
            board_id: "board_1".into(),
            event_id: "evt_1".into(),
            nats_seq: 1,
            board_revision: 1,
            emitted_at: buffa_types::google::protobuf::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
                ..Default::default()
            }
            .into(),
            emitter_pod_id: "pod_1".into(),
            actor_subject: "user:1".into(),
            payload,
            ..Default::default()
        }
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

    fn heartbeat_envelope() -> Vec<u8> {
        v1::SubscribeBoardResponse {
            envelope: envelope(Some(Payload::Heartbeat(Box::new(v1::Heartbeat {
                server_time_ms: 1_700_000_000_000,
                ..Default::default()
            }))))
            .into(),
            ..Default::default()
        }
        .encode_to_vec()
    }

    #[tokio::test]
    async fn subscribe_board_streams_envelopes() {
        let server = MockServer::start().await;
        let card_created = v1::SubscribeBoardResponse {
            envelope: envelope(Some(Payload::CardCreated(Box::new(v1::CardCreated {
                card: v1::Card {
                    id: "card_1".into(),
                    board_id: "board_1".into(),
                    column_id: "col_1".into(),
                    r#ref: "BEAM-1".into(),
                    title: "Fix crash".into(),
                    labels: vec![v1::Label {
                        id: "label_1".into(),
                        name: "bug".into(),
                        ..Default::default()
                    }],
                    assignees: vec![v1::Assignee {
                        subject: "user:1".into(),
                        display_name: "Ada".into(),
                        ..Default::default()
                    }],
                    checklist: vec![v1::ChecklistItem {
                        id: "chk_1".into(),
                        text: "Reproduce".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }
                .into(),
                column_id: "col_1".into(),
                position: 1,
                idempotency_key: "ik".into(),
                ..Default::default()
            }))))
            .into(),
            ..Default::default()
        }
        .encode_to_vec();
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/SubscribeBoard"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+proto")
                    .set_body_bytes(connect_stream_body(&[heartbeat_envelope(), card_created])),
            )
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            SubscribeAction::Board {
                board_id: "board_1".into(),
            },
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn subscribe_project_sets_object_id_header() {
        let server = MockServer::start().await;
        let body = v1::SubscribeProjectResponse {
            envelope: envelope(None).into(),
            ..Default::default()
        }
        .encode_to_vec();
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/SubscribeProject"))
            .and(header("x-sunbeam-object-id", "proj_1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+proto")
                    .set_body_bytes(connect_stream_body(&[body])),
            )
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            SubscribeAction::Project {
                project_id: "proj_1".into(),
            },
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn subscribe_board_rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/SubscribeBoard"))
            .respond_with(testutil::connect_error(500, "internal", "nats down"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            SubscribeAction::Board {
                board_id: "board_1".into(),
            },
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("nats down"));
    }

    #[tokio::test]
    async fn subscribe_board_stream_error_propagates() {
        let server = MockServer::start().await;
        let end = br#"{"error":{"code":"internal","message":"stream boom"}}"#;
        let mut body = connect_stream_body(&[heartbeat_envelope()]);
        // Replace the trailer with an end-of-stream frame carrying an error.
        body.truncate(body.len() - 7);
        body.push(2u8); // flags: end-of-stream
        body.extend_from_slice(&(end.len() as u32).to_be_bytes());
        body.extend_from_slice(end);
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/SubscribeBoard"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+proto")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            SubscribeAction::Board {
                board_id: "board_1".into(),
            },
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("stream boom"));
    }

    #[tokio::test]
    async fn envelope_without_payload_renders_null_payload() {
        let value = envelope_to_json(&envelope(None)).await;
        assert_eq!(value["payload"], Value::Null);
        assert_eq!(value["board_id"], "board_1");
        assert_eq!(value["event_id"], "evt_1");
        assert_eq!(value["nats_seq"], 1);
        assert!(value["emitted_at"].as_str().unwrap().contains("T"));
    }

    #[tokio::test]
    async fn all_payload_variants_render_with_type() {
        use Payload::*;
        let column = v1::EventColumn {
            id: "col".into(),
            board_id: "b".into(),
            title: "Col".into(),
            ..Default::default()
        };
        let board = v1::EventBoard {
            id: "b".into(),
            project_id: "p".into(),
            name: "B".into(),
            ..Default::default()
        };
        let card = v1::Card {
            id: "c1".into(),
            board_id: "b".into(),
            column_id: "col".into(),
            r#ref: "REF-1".into(),
            title: "t".into(),
            priority: v1::CardPriority::Low.into(),
            urgency: v1::CardUrgency::Low.into(),
            github_links: vec![v1::GitHubLink {
                id: "g1".into(),
                repo: "r".into(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let cases: Vec<(Payload, &str)> = vec![
            (
                CardCreated(Box::new(v1::CardCreated {
                    card: card.into(),
                    column_id: "col".into(),
                    ..Default::default()
                })),
                "CardCreated",
            ),
            (
                CardUpdated(Box::new(v1::CardUpdated {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "CardUpdated",
            ),
            (
                CardMoved(Box::new(v1::CardMoved {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "CardMoved",
            ),
            (
                CardTransferred(Box::new(v1::CardTransferred {
                    card: v1::Card {
                        id: "c1".into(),
                        ..Default::default()
                    }
                    .into(),
                    ..Default::default()
                })),
                "CardTransferred",
            ),
            (
                CardDeleted(Box::new(v1::CardDeleted {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "CardDeleted",
            ),
            (
                ColumnAdded(Box::new(v1::ColumnAdded {
                    column: column.clone().into(),
                    ..Default::default()
                })),
                "ColumnAdded",
            ),
            (
                ColumnRenamed(Box::new(v1::ColumnRenamed {
                    column_id: "col".into(),
                    ..Default::default()
                })),
                "ColumnRenamed",
            ),
            (
                ColumnRemoved(Box::new(v1::ColumnRemoved {
                    column_id: "col".into(),
                    ..Default::default()
                })),
                "ColumnRemoved",
            ),
            (
                ColumnUpdated(Box::new(v1::ColumnUpdated {
                    column: column.clone().into(),
                    ..Default::default()
                })),
                "ColumnUpdated",
            ),
            (
                ColumnsReordered(Box::new(v1::ColumnsReordered {
                    columns: vec![column],
                    ..Default::default()
                })),
                "ColumnsReordered",
            ),
            (
                BoardRenamed(Box::new(v1::BoardRenamed {
                    new_name: "N".into(),
                    ..Default::default()
                })),
                "BoardRenamed",
            ),
            (
                BoardUpdated(Box::new(v1::BoardUpdated {
                    board: board.into(),
                    ..Default::default()
                })),
                "BoardUpdated",
            ),
            (
                MembershipChanged(Box::new(v1::MembershipChanged {
                    subject: "s".into(),
                    ..Default::default()
                })),
                "MembershipChanged",
            ),
            (
                MemberAdded(Box::new(v1::MemberAdded {
                    subject: "s".into(),
                    ..Default::default()
                })),
                "MemberAdded",
            ),
            (
                MemberRemoved(Box::new(v1::MemberRemoved {
                    subject: "s".into(),
                    ..Default::default()
                })),
                "MemberRemoved",
            ),
            (
                MemberRoleChanged(Box::new(v1::MemberRoleChanged {
                    subject: "s".into(),
                    ..Default::default()
                })),
                "MemberRoleChanged",
            ),
            (
                BoardCreated(Box::new(v1::BoardCreated {
                    board_id: "b".into(),
                    ..Default::default()
                })),
                "BoardCreated",
            ),
            (
                BoardDeleted(Box::new(v1::BoardDeleted {
                    board_id: "b".into(),
                    ..Default::default()
                })),
                "BoardDeleted",
            ),
            (
                ProjectUpdated(Box::new(v1::ProjectUpdated {
                    project_id: "p".into(),
                    ..Default::default()
                })),
                "ProjectUpdated",
            ),
            (
                ChecklistUpdated(Box::new(v1::ChecklistUpdated {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "ChecklistUpdated",
            ),
            (
                CommentAdded(Box::new(v1::CommentAdded {
                    card_id: "c1".into(),
                    author_sub: "a".into(),
                    ..Default::default()
                })),
                "CommentAdded",
            ),
            (
                CommentEdited(Box::new(v1::CommentEdited {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "CommentEdited",
            ),
            (
                CommentDeleted(Box::new(v1::CommentDeleted {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "CommentDeleted",
            ),
            (
                AttachmentAdded(Box::new(v1::AttachmentAdded {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "AttachmentAdded",
            ),
            (
                AttachmentDeleted(Box::new(v1::AttachmentDeleted {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "AttachmentDeleted",
            ),
            (
                GithubLinkAdded(Box::new(v1::GitHubLinkAdded {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "GitHubLinkAdded",
            ),
            (
                GithubLinkRefreshed(Box::new(v1::GitHubLinkRefreshed {
                    card_id: "c1".into(),
                    ..Default::default()
                })),
                "GitHubLinkRefreshed",
            ),
            (
                AggregatedBoardCreated(Box::new(v1::AggregatedBoardCreated {
                    aggregated_board_id: "ab".into(),
                    ..Default::default()
                })),
                "AggregatedBoardCreated",
            ),
            (
                AggregatedBoardUpdated(Box::new(v1::AggregatedBoardUpdated {
                    aggregated_board_id: "ab".into(),
                    ..Default::default()
                })),
                "AggregatedBoardUpdated",
            ),
            (
                AggregatedBoardDeleted(Box::new(v1::AggregatedBoardDeleted {
                    aggregated_board_id: "ab".into(),
                    ..Default::default()
                })),
                "AggregatedBoardDeleted",
            ),
            (
                SourceBoardAdded(Box::new(v1::SourceBoardAdded {
                    aggregated_board_id: "ab".into(),
                    ..Default::default()
                })),
                "SourceBoardAdded",
            ),
            (
                SourceBoardRemoved(Box::new(v1::SourceBoardRemoved {
                    aggregated_board_id: "ab".into(),
                    ..Default::default()
                })),
                "SourceBoardRemoved",
            ),
            (
                Heartbeat(Box::new(v1::Heartbeat {
                    server_time_ms: 1,
                    ..Default::default()
                })),
                "Heartbeat",
            ),
            (
                Cutover(Box::new(v1::Cutover {
                    last_replay_nats_seq: 42,
                    ..Default::default()
                })),
                "Cutover",
            ),
        ];

        assert_eq!(cases.len(), 34);
        for (payload, expected_type) in cases {
            let value = payload_to_json(&payload).await;
            assert_eq!(value["type"], expected_type, "wrong type discriminator");
        }
    }

    #[tokio::test]
    async fn card_created_enriches_assignee_emails() {
        let payload = Payload::CardCreated(Box::new(v1::CardCreated {
            card: v1::Card {
                id: "c1".into(),
                assignees: vec![
                    // Server-populated email (kanban v2026.07.12+): used
                    // as-is, no identity lookup.
                    v1::Assignee {
                        subject: "user:1".into(),
                        email: "one@example.com".into(),
                        ..Default::default()
                    },
                    // Empty email: falls back to the identity lookup, which
                    // is unconfigured in tests and resolves to null.
                    v1::Assignee {
                        subject: "user:2".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }
            .into(),
            ..Default::default()
        }));
        let value = payload_to_json(&payload).await;
        assert_eq!(
            value["card"]["assignees"][0]["email"],
            json!("one@example.com")
        );
        assert_eq!(value["card"]["assignees"][1]["email"], Value::Null);
    }

    #[test]
    fn typed_inserts_discriminator() {
        let value = typed("X", json!({"a": 1}));
        assert_eq!(value["type"], "X");
        assert_eq!(value["a"], 1);
        assert_eq!(typed("Y", Value::Null), Value::Null);
    }
}
