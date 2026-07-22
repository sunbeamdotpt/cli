//! Kanban card commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, new_idempotency_key, object_id_options, required};
use crate::output::{OutputFormat, render, render_list};

/// Card actions.
#[derive(Debug, Subcommand)]
pub enum CardAction {
    /// List cards.
    List {
        /// Board ID or name.
        #[arg(short, long)]
        board: String,
        /// Column ID.
        #[arg(short, long)]
        column: Option<String>,
    },
    /// Get a card.
    Get {
        /// Card ID, title, or ref.
        card_id: String,
    },
    /// Create a card.
    Create {
        /// Board ID or name.
        #[arg(short, long)]
        board: String,
        /// Column ID.
        #[arg(short, long)]
        column: Option<String>,
        /// Title.
        #[arg(short, long)]
        title: String,
        /// Description.
        #[arg(short, long)]
        description: Option<String>,
        /// Priority.
        #[arg(short, long, value_enum)]
        priority: Option<PriorityArg>,
    },
    /// Update a card.
    Update {
        /// Card ID, title, or ref.
        card_id: String,
        /// New title.
        #[arg(short, long)]
        title: Option<String>,
        /// New description.
        #[arg(short, long)]
        description: Option<String>,
        /// New priority.
        #[arg(short, long, value_enum)]
        priority: Option<PriorityArg>,
    },
    /// Move a card.
    Move {
        /// Card ID, title, or ref.
        card_id: String,
        /// Destination column ID.
        #[arg(short, long)]
        column: String,
        /// Position within the column.
        #[arg(short, long)]
        position: Option<i32>,
    },
    /// Delete a card.
    Delete {
        /// Card ID, title, or ref.
        card_id: String,
    },
    /// Dependency management.
    Dependency {
        /// Dependency subcommand to run.
        #[command(subcommand)]
        action: DependencyAction,
    },
}

/// Priority values matching `CardPriority`.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PriorityArg {
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Urgent.
    Urgent,
}

impl From<PriorityArg> for v1::CardPriority {
    fn from(p: PriorityArg) -> Self {
        match p {
            PriorityArg::Low => v1::CardPriority::Low,
            PriorityArg::Medium => v1::CardPriority::Medium,
            PriorityArg::High => v1::CardPriority::High,
            PriorityArg::Urgent => v1::CardPriority::Urgent,
        }
    }
}

/// Lowercase display name for a `CardPriority` value.
pub(crate) fn priority_name(value: i32) -> String {
    match value {
        1 => "low".to_string(),
        2 => "medium".to_string(),
        3 => "high".to_string(),
        4 => "urgent".to_string(),
        _ => "unspecified".to_string(),
    }
}

/// Lowercase display name for a `CardUrgency` value.
pub(crate) fn urgency_name(value: i32) -> String {
    match value {
        1 => "low".to_string(),
        2 => "medium".to_string(),
        3 => "high".to_string(),
        4 => "critical".to_string(),
        _ => "unspecified".to_string(),
    }
}

/// Card dependency actions.
#[derive(Debug, Subcommand)]
pub enum DependencyAction {
    /// Add a dependency.
    Add {
        /// Board ID or name.
        #[arg(short, long)]
        board: String,
        /// Card ID, title, or ref.
        card_id: String,
        /// Card this card depends on (ID, title, or ref).
        depends_on: String,
    },
    /// Remove a dependency.
    Remove {
        /// Board ID or name.
        #[arg(short, long)]
        board: String,
        /// Card ID, title, or ref.
        card_id: String,
        /// Dependency card ID, title, or ref.
        depends_on: String,
    },
}

/// Serializable card summary for list views.
#[derive(Serialize)]
struct CardOut {
    id: String,
    r#ref: String,
    board_id: String,
    column_id: String,
    title: String,
    priority: String,
    blocked: bool,
    position: i32,
    comments_count: i32,
    attachments_count: i32,
}

impl From<v1::Card> for CardOut {
    fn from(card: v1::Card) -> Self {
        Self {
            id: card.id,
            r#ref: card.r#ref,
            board_id: card.board_id,
            column_id: card.column_id,
            title: card.title,
            priority: priority_name(card.priority.to_i32()),
            blocked: card.blocked,
            position: card.position,
            comments_count: card.comments_count,
            attachments_count: card.attachments_count,
        }
    }
}

/// Serializable card detail for get/create/update/move/dependency views.
#[derive(Serialize)]
pub(crate) struct CardDetailOut {
    id: String,
    r#ref: String,
    project_id: String,
    board_id: String,
    column_id: String,
    title: String,
    description: String,
    priority: String,
    urgency: String,
    due: String,
    completed_at: String,
    blocked: bool,
    cover: String,
    milestone_id: String,
    position: i32,
    labels: Vec<serde_json::Value>,
    assignees: Vec<serde_json::Value>,
    checklist: Vec<serde_json::Value>,
    github_links: Vec<serde_json::Value>,
    comments_count: i32,
    attachments_count: i32,
    revision: u64,
    created_at: String,
    updated_at: String,
    depends_on_card_ids: Vec<String>,
    dependent_card_ids: Vec<String>,
}

impl From<v1::Card> for CardDetailOut {
    fn from(card: v1::Card) -> Self {
        Self {
            id: card.id,
            r#ref: card.r#ref,
            project_id: card.project_id,
            board_id: card.board_id,
            column_id: card.column_id,
            title: card.title,
            description: card.description,
            priority: priority_name(card.priority.to_i32()),
            urgency: urgency_name(card.urgency.to_i32()),
            due: fmt_ts(&card.due),
            completed_at: fmt_ts(&card.completed_at),
            blocked: card.blocked,
            cover: card.cover,
            milestone_id: card.milestone_id,
            position: card.position,
            labels: card
                .labels
                .into_iter()
                .map(|l| {
                    serde_json::json!({
                        "id": l.id,
                        "name": l.name,
                        "style": l.style,
                    })
                })
                .collect(),
            assignees: card
                .assignees
                .into_iter()
                .map(|a| {
                    serde_json::json!({
                        "subject": a.subject,
                        "display_name": a.display_name,
                        "avatar_url": a.avatar_url,
                        "email": null,
                    })
                })
                .collect(),
            checklist: card
                .checklist
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "text": c.text,
                        "done": c.done,
                        "position": c.position,
                    })
                })
                .collect(),
            github_links: card
                .github_links
                .into_iter()
                .map(|g| {
                    serde_json::json!({
                        "id": g.id,
                        "repo": g.repo,
                        "number": g.number,
                        "state": g.state,
                        "merged": g.merged,
                    })
                })
                .collect(),
            comments_count: card.comments_count,
            attachments_count: card.attachments_count,
            revision: card.revision,
            created_at: fmt_ts(&card.created_at),
            updated_at: fmt_ts(&card.updated_at),
            depends_on_card_ids: card.depends_on_card_ids,
            dependent_card_ids: card.dependent_card_ids,
        }
    }
}

/// Resolve assignee subjects to email addresses through the sso-gateway
/// IdentityService.
///
/// Missing or unresolvable subjects are left as `null` rather than
/// failing the whole command.
async fn resolve_assignee_emails(assignees: &mut [serde_json::Value]) -> Result<()> {
    let subjects: Vec<&str> = assignees
        .iter()
        .filter_map(|a| {
            a.get("subject")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .collect();

    if subjects.is_empty() {
        return Ok(());
    }

    let email_map = crate::auth::resolve_emails_for_subjects(&subjects).await?;

    for assignee in assignees.iter_mut() {
        let Some(subject) = assignee
            .get("subject")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if let Some(email) = email_map.get(subject)
            && let Some(obj) = assignee.as_object_mut()
        {
            obj.insert(
                "email".to_string(),
                serde_json::Value::String(email.clone()),
            );
        }
    }
    Ok(())
}

/// Render a card detail, resolving assignee emails first.
async fn render_card_detail(card: v1::Card, format: OutputFormat) -> Result<()> {
    let mut detail = CardDetailOut::from(card);
    resolve_assignee_emails(&mut detail.assignees).await?;
    render(&detail, format)
}

/// Run a card command.
pub(crate) async fn run(
    cmd: CardAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        CardAction::List { board, column } => {
            let resp = client
                .cards()
                .list_cards_by_board_with_options(
                    v1::ListCardsByBoardRequest {
                        board_id: board.clone(),
                        column_id: column.unwrap_or_default(),
                        ..Default::default()
                    },
                    object_id_options(&board),
                )
                .await?
                .into_owned();
            let cards: Vec<_> = resp.cards.into_iter().map(CardOut::from).collect();
            render_list(
                &cards,
                &[
                    "REF", "COLUMN", "TITLE", "PRIORITY", "BLOCKED", "POSITION", "ID",
                ],
                |c| {
                    vec![
                        c.r#ref.clone(),
                        c.column_id.clone(),
                        c.title.clone(),
                        c.priority.clone(),
                        c.blocked.to_string(),
                        c.position.to_string(),
                        c.id.clone(),
                    ]
                },
                format,
            )
        }
        CardAction::Get { card_id } => {
            let resp = client
                .cards()
                .get_card_with_options(
                    v1::GetCardRequest {
                        card_id: card_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&card_id),
                )
                .await?
                .into_owned();
            render_card_detail(required(resp.card, "card")?, format).await
        }
        CardAction::Create {
            board,
            column,
            title,
            description,
            priority,
        } => {
            let resp = client
                .cards()
                .create_card_with_options(
                    v1::CreateCardRequest {
                        board_id: board.clone(),
                        column_id: column.unwrap_or_default(),
                        title,
                        description: description.unwrap_or_default(),
                        priority: priority
                            .map(v1::CardPriority::from)
                            .unwrap_or(v1::CardPriority::Unspecified)
                            .into(),
                        milestone_id: String::new(),
                        position: 0,
                        idempotency_key: new_idempotency_key(),
                        urgency: v1::CardUrgency::Medium.into(),
                        ..Default::default()
                    },
                    object_id_options(&board),
                )
                .await?
                .into_owned();
            render_card_detail(required(resp.card, "card")?, format).await
        }
        CardAction::Update {
            card_id,
            title,
            description,
            priority,
        } => {
            let mut update_card = v1::Card {
                id: card_id.clone(),
                ..Default::default()
            };
            let mut paths: Vec<String> = Vec::new();
            if let Some(t) = title {
                update_card.title = t;
                paths.push("title".to_string());
            }
            if let Some(d) = description {
                update_card.description = d;
                paths.push("description".to_string());
            }
            if let Some(p) = priority {
                update_card.priority = v1::CardPriority::from(p).into();
                paths.push("priority".to_string());
            }
            let resp = client
                .cards()
                .update_card_with_options(
                    v1::UpdateCardRequest {
                        card_id: card_id.clone(),
                        card: update_card.into(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths,
                            ..Default::default()
                        })
                        .into(),
                        idempotency_key: new_idempotency_key(),
                        ..Default::default()
                    },
                    object_id_options(&card_id),
                )
                .await?
                .into_owned();
            render_card_detail(required(resp.card, "card")?, format).await
        }
        CardAction::Move {
            card_id,
            column,
            position,
        } => {
            let resp = client
                .cards()
                .move_card_with_options(
                    v1::MoveCardRequest {
                        card_id: card_id.clone(),
                        to_column_id: column,
                        to_position: position.unwrap_or_default(),
                        idempotency_key: new_idempotency_key(),
                        ..Default::default()
                    },
                    object_id_options(&card_id),
                )
                .await?
                .into_owned();
            render_card_detail(required(resp.card, "card")?, format).await
        }
        CardAction::Delete { card_id } => {
            client
                .cards()
                .delete_card_with_options(
                    v1::DeleteCardRequest {
                        card_id: card_id.clone(),
                        ..Default::default()
                    },
                    object_id_options(&card_id),
                )
                .await?;
            Ok(())
        }
        CardAction::Dependency { action } => match action {
            DependencyAction::Add {
                board,
                card_id,
                depends_on,
            } => {
                let resp = client
                    .cards()
                    .add_card_dependency_with_options(
                        v1::AddCardDependencyRequest {
                            card_id: card_id.clone(),
                            depends_on_card_id: depends_on.clone(),
                            idempotency_key: new_idempotency_key(),
                            ..Default::default()
                        },
                        object_id_options(&board),
                    )
                    .await?
                    .into_owned();
                render_card_detail(required(resp.card, "card")?, format).await
            }
            DependencyAction::Remove {
                board,
                card_id,
                depends_on,
            } => {
                let resp = client
                    .cards()
                    .remove_card_dependency_with_options(
                        v1::RemoveCardDependencyRequest {
                            card_id: card_id.clone(),
                            depends_on_card_id: depends_on.clone(),
                            idempotency_key: new_idempotency_key(),
                            ..Default::default()
                        },
                        object_id_options(&board),
                    )
                    .await?
                    .into_owned();
                render_card_detail(required(resp.card, "card")?, format).await
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

    fn card(id: &str, title: &str) -> v1::Card {
        v1::Card {
            id: id.to_string(),
            project_id: "proj_1".into(),
            board_id: "board_1".into(),
            column_id: "col_1".into(),
            r#ref: "BEAM-1".into(),
            title: title.to_string(),
            priority: v1::CardPriority::High.into(),
            urgency: v1::CardUrgency::Critical.into(),
            position: 1,
            revision: 3,
            labels: vec![v1::Label {
                id: "label_1".into(),
                project_id: "proj_1".into(),
                name: "bug".into(),
                style: "red".into(),
                ..Default::default()
            }],
            assignees: vec![v1::Assignee {
                subject: "user:alice".into(),
                display_name: "Alice".into(),
                ..Default::default()
            }],
            checklist: vec![v1::ChecklistItem {
                id: "chk_1".into(),
                text: "repro".into(),
                done: true,
                position: 1,
                ..Default::default()
            }],
            github_links: vec![v1::GitHubLink {
                id: "gh_1".into(),
                repo: "sunbeam/cli".into(),
                number: 42,
                state: "open".into(),
                merged: false,
                ..Default::default()
            }],
            depends_on_card_ids: vec!["card_0".into()],
            dependent_card_ids: vec!["card_2".into()],
            ..Default::default()
        }
    }

    #[test]
    fn priority_and_urgency_names() {
        assert_eq!(priority_name(1), "low");
        assert_eq!(priority_name(4), "urgent");
        assert_eq!(priority_name(0), "unspecified");
        assert_eq!(urgency_name(4), "critical");
        assert_eq!(urgency_name(9), "unspecified");
        assert_eq!(
            buffa::EnumValue::from(v1::CardPriority::from(PriorityArg::Urgent)).to_i32(),
            4
        );
    }

    fn lean_card(id: &str, title: &str) -> v1::Card {
        v1::Card {
            id: id.to_string(),
            project_id: "proj_1".into(),
            board_id: "board_1".into(),
            column_id: "col_1".into(),
            r#ref: "BEAM-1".into(),
            title: title.to_string(),
            priority: v1::CardPriority::High.into(),
            position: 1,
            ..Default::default()
        }
    }

    #[test]
    fn card_detail_conversion_covers_nested_fields() {
        let detail = CardDetailOut::from(card("card_1", "Fix crash"));
        assert_eq!(detail.r#ref, "BEAM-1");
        assert_eq!(detail.priority, "high");
        assert_eq!(detail.urgency, "critical");
        assert_eq!(detail.labels.len(), 1);
        assert_eq!(detail.assignees.len(), 1);
        assert_eq!(detail.checklist.len(), 1);
        assert_eq!(detail.github_links.len(), 1);
        assert_eq!(detail.depends_on_card_ids, vec!["card_0".to_string()]);
    }

    #[tokio::test]
    async fn list_cards_all_formats() {
        for format in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Yaml] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/sunbeam.kanban.v1.CardService/ListCardsByBoard"))
                .respond_with(testutil::proto_response(&v1::ListCardsByBoardResponse {
                    cards: vec![lean_card("card_1", "Fix crash")],
                    ..Default::default()
                }))
                .mount(&server)
                .await;

            let client = testutil::client_for(&server.uri());
            run(
                CardAction::List {
                    board: "board_1".into(),
                    column: None,
                },
                format,
                &client,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn get_card() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::proto_response(&v1::GetCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Get {
                card_id: "card_1".into(),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_update_move_delete_card() {
        let server = MockServer::start().await;
        for (rpc, body) in [
            (
                "CreateCard",
                testutil::proto_response(&v1::CreateCardResponse {
                    card: lean_card("card_1", "Fix crash").into(),
                    ..Default::default()
                }),
            ),
            (
                "UpdateCard",
                testutil::proto_response(&v1::UpdateCardResponse {
                    card: lean_card("card_1", "Fix crash").into(),
                    ..Default::default()
                }),
            ),
            (
                "MoveCard",
                testutil::proto_response(&v1::MoveCardResponse {
                    card: lean_card("card_1", "Fix crash").into(),
                    ..Default::default()
                }),
            ),
        ] {
            Mock::given(method("POST"))
                .and(path(format!("/sunbeam.kanban.v1.CardService/{rpc}")))
                .respond_with(body)
                .mount(&server)
                .await;
        }
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/DeleteCard"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Create {
                board: "board_1".into(),
                column: Some("col_1".into()),
                title: "Fix crash".into(),
                description: Some("details".into()),
                priority: Some(PriorityArg::High),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Update {
                card_id: "card_1".into(),
                title: Some("New title".into()),
                description: None,
                priority: Some(PriorityArg::Low),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Move {
                card_id: "card_1".into(),
                column: "col_2".into(),
                position: Some(3),
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Delete {
                card_id: "card_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dependency_add_and_remove() {
        let server = MockServer::start().await;
        for (rpc, body) in [
            (
                "AddCardDependency",
                testutil::proto_response(&v1::AddCardDependencyResponse {
                    card: lean_card("card_1", "Fix crash").into(),
                    ..Default::default()
                }),
            ),
            (
                "RemoveCardDependency",
                testutil::proto_response(&v1::RemoveCardDependencyResponse {
                    card: lean_card("card_1", "Fix crash").into(),
                    ..Default::default()
                }),
            ),
        ] {
            Mock::given(method("POST"))
                .and(path(format!("/sunbeam.kanban.v1.CardService/{rpc}")))
                .respond_with(body)
                .mount(&server)
                .await;
        }

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Dependency {
                action: DependencyAction::Add {
                    board: "board_1".into(),
                    card_id: "card_1".into(),
                    depends_on: "card_0".into(),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Dependency {
                action: DependencyAction::Remove {
                    board: "board_1".into(),
                    card_id: "card_1".into(),
                    depends_on: "card_0".into(),
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rpc_error_is_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::connect_error(404, "not_found", "card gone"))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            CardAction::Get {
                card_id: "card_1".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("card gone"));
    }
}
