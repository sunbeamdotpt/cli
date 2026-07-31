//! Kanban card commands.

use clap::Subcommand;
use sdk::error::Result;
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::buffa_types;
use sdk::kanban::v1;
use serde::Serialize;

use super::{fmt_ts, mutating_options, new_idempotency_key, object_id_options, required};
use crate::output::{OutputFormat, render, render_list, truncate};

/// Card actions.
#[derive(Debug, Clone, Subcommand)]
pub enum CardAction {
    /// List cards.
    List {
        /// Board ID or name.
        board: String,
        /// Column ID or title.
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
        board: String,
        /// Column ID or title (defaults to the board's left-most column).
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
        /// Urgency (defaults to medium).
        #[arg(short, long, value_enum)]
        urgency: Option<UrgencyArg>,
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
        /// New urgency.
        #[arg(short, long, value_enum)]
        urgency: Option<UrgencyArg>,
        /// Mark the card as blocked.
        #[arg(long)]
        blocked: bool,
        /// Clear the blocked flag.
        #[arg(long, conflicts_with = "blocked")]
        unblocked: bool,
        /// Assign a milestone (ID or title; titles resolve against the
        /// card's project).
        #[arg(long)]
        milestone: Option<String>,
    },
    /// Assign a card to a user.
    Assign {
        /// Card ID, title, or ref.
        card_id: String,
        /// User email address or identity ULID (`user:<ulid>`).
        subject: String,
    },
    /// Unassign a card from a user.
    Unassign {
        /// Card ID, title, or ref.
        card_id: String,
        /// User email address or identity ULID (`user:<ulid>`).
        subject: String,
    },
    /// Move a card. Moves within the card's board by default; pass --board
    /// to relocate it to another board (recreate-and-close).
    Move {
        /// Card ID, title, or ref.
        card_id: String,
        /// Destination column ID or title.
        #[arg(short, long)]
        column: String,
        /// Position within the column.
        #[arg(short, long)]
        position: Option<i32>,
        /// Destination board for a cross-board move (ID, name, or
        /// project/board). The card is recreated there and the original is
        /// deleted.
        #[arg(short, long)]
        board: Option<String>,
    },
    /// Delete a card.
    Delete {
        /// Card ID, title, or ref.
        card_id: String,
    },
    /// Comment management.
    Comment {
        /// Comment subcommand to run.
        #[command(subcommand)]
        action: CommentAction,
    },
    /// Label assignment.
    Label {
        /// Label subcommand to run.
        #[command(subcommand)]
        action: CardLabelAction,
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

/// Urgency values matching `CardUrgency`.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum UrgencyArg {
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Critical.
    Critical,
}

impl From<UrgencyArg> for v1::CardUrgency {
    fn from(u: UrgencyArg) -> Self {
        match u {
            UrgencyArg::Low => v1::CardUrgency::Low,
            UrgencyArg::Medium => v1::CardUrgency::Medium,
            UrgencyArg::High => v1::CardUrgency::High,
            UrgencyArg::Critical => v1::CardUrgency::Critical,
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
#[derive(Debug, Clone, Subcommand)]
pub enum DependencyAction {
    /// Add a dependency.
    Add {
        /// Board ID or name.
        board: String,
        /// Card ID, title, or ref.
        card_id: String,
        /// Card this card depends on (ID, title, or ref).
        depends_on: String,
    },
    /// Remove a dependency.
    Remove {
        /// Board ID or name.
        board: String,
        /// Card ID, title, or ref.
        card_id: String,
        /// Dependency card ID, title, or ref.
        depends_on: String,
    },
}

/// Card label actions.
#[derive(Debug, Clone, Subcommand)]
pub enum CardLabelAction {
    /// Replace a card's whole label set (no names clears all labels).
    Set {
        /// Card ID, title, or ref.
        card_id: String,
        /// Label names or IDs.
        names: Vec<String>,
    },
    /// Add labels to a card.
    Add {
        /// Card ID, title, or ref.
        card_id: String,
        /// Label names or IDs.
        names: Vec<String>,
    },
    /// Remove labels from a card.
    Remove {
        /// Card ID, title, or ref.
        card_id: String,
        /// Label names or IDs.
        names: Vec<String>,
    },
}

/// How a card label operation combines with the current label set.
enum LabelMode {
    Set,
    Add,
    Remove,
}

/// Card comment actions.
#[derive(Debug, Clone, Subcommand)]
pub enum CommentAction {
    /// List comments on a card.
    List {
        /// Card ID, title, or ref.
        card_id: String,
    },
    /// Add a comment to a card.
    Add {
        /// Card ID, title, or ref.
        card_id: String,
        /// Comment body (markdown).
        #[arg(short, long)]
        message: String,
    },
    /// Edit a comment.
    Edit {
        /// Card ID, title, or ref.
        card_id: String,
        /// Comment ID.
        comment_id: String,
        /// New comment body (markdown).
        #[arg(short, long)]
        message: String,
    },
    /// Delete a comment.
    Delete {
        /// Card ID, title, or ref.
        card_id: String,
        /// Comment ID.
        comment_id: String,
    },
}

/// Serializable comment for output.
#[derive(Serialize)]
struct CommentOut {
    id: String,
    card_id: String,
    author_sub: String,
    body: String,
    created_at: String,
    updated_at: String,
}

impl From<v1::Comment> for CommentOut {
    fn from(c: v1::Comment) -> Self {
        Self {
            id: c.id,
            card_id: c.card_id,
            author_sub: c.author_sub,
            body: c.body,
            created_at: fmt_ts(&c.created_at),
            updated_at: fmt_ts(&c.updated_at),
        }
    }
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
    assignees: Vec<String>,
    comments_count: i32,
    attachments_count: i32,
    created_at: String,
    updated_at: String,
    completed_at: String,
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
            assignees: card
                .assignees
                .into_iter()
                .map(|a| {
                    if a.display_name.is_empty() {
                        a.subject
                    } else {
                        a.display_name
                    }
                })
                .collect(),
            comments_count: card.comments_count,
            attachments_count: card.attachments_count,
            created_at: fmt_ts(&card.created_at),
            updated_at: fmt_ts(&card.updated_at),
            completed_at: fmt_ts(&card.completed_at),
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

/// Resolve an assignee argument to an OIDC subject.
///
/// Values containing `@` are treated as email addresses and resolved through
/// the sso-gateway; ULID-shaped values (bare or `user:`-prefixed) are
/// verified via GetIdentity; anything else is rejected locally.
async fn resolve_subject(raw: &str) -> Result<String> {
    if raw.contains('@') {
        crate::auth::resolve_subject_for_email(raw).await
    } else {
        crate::auth::resolve_verified_subject(raw).await
    }
}

/// Resolve a `--milestone` argument to a milestone ULID.
///
/// ID-shaped input passes through; titles resolve against the milestones of
/// the card's project.
async fn resolve_milestone_arg(client: &KanbanClient, card_id: &str, raw: &str) -> Result<String> {
    if super::resolve::looks_like_id(raw) {
        return Ok(raw.to_string());
    }
    let card = required(
        client
            .cards()
            .get_card_with_options(
                v1::GetCardRequest {
                    card_id: card_id.to_string(),
                    ..Default::default()
                },
                object_id_options(card_id),
            )
            .await?
            .into_owned()
            .card,
        "card",
    )?;
    super::resolve::NameResolver::new(client)
        .milestone(&card.project_id, raw)
        .await
}

/// Apply a card label operation against the project catalog.
///
/// Label names (and ULIDs) resolve against the catalog visible to the card's
/// project; the resulting set is pushed through BulkUpdateCardLabels, which
/// replaces a card's labels wholesale.
async fn update_card_labels(
    client: &KanbanClient,
    card_id: &str,
    names: &[String],
    mode: LabelMode,
    format: OutputFormat,
) -> Result<()> {
    let card = required(
        client
            .cards()
            .get_card_with_options(
                v1::GetCardRequest {
                    card_id: card_id.to_string(),
                    ..Default::default()
                },
                object_id_options(card_id),
            )
            .await?
            .into_owned()
            .card,
        "card",
    )?;
    let catalog = super::resolve::NameResolver::new(client)
        .project_labels(&card.project_id)
        .await?;
    let available: Vec<String> = catalog.iter().map(|l| l.name.clone()).collect();

    let mut wanted: Vec<String> = Vec::new();
    for name in names {
        if super::resolve::looks_like_id(name) {
            wanted.push(name.clone());
            continue;
        }
        let matches: Vec<_> = catalog
            .iter()
            .filter(|l| super::resolve::name_matches(&l.name, name))
            .map(|l| (l.id.clone(), l.name.clone()))
            .collect();
        wanted.push(super::resolve::unique_match(
            matches, "label", name, &available,
        )?);
    }

    let current: Vec<String> = card.labels.iter().map(|l| l.id.clone()).collect();
    let next: Vec<String> = match mode {
        LabelMode::Set => wanted,
        LabelMode::Add => {
            let mut ids = current;
            for id in wanted {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        }
        LabelMode::Remove => current
            .into_iter()
            .filter(|id| !wanted.contains(id))
            .collect(),
    };

    let resp = client
        .cards()
        .bulk_update_card_labels_with_options(
            v1::BulkUpdateCardLabelsRequest {
                card_ids: vec![card_id.to_string()],
                label_ids: next,
                idempotency_key: new_idempotency_key(),
                ..Default::default()
            },
            // BulkUpdateCardLabels is gated on KanbanBoard + edit, so the
            // object id must be the board's, not the card's.
            object_id_options(&card.board_id),
        )
        .await?
        .into_owned();
    let updated = resp
        .cards
        .into_iter()
        .next()
        .ok_or_else(|| sdk::error::SunbeamError::network("server response missing card"))?;
    render_card_detail(updated, format).await
}

/// Move a card to another column on its current board.
async fn move_card_within_board(
    client: &KanbanClient,
    card_id: &str,
    column: &str,
    position: Option<i32>,
    format: OutputFormat,
) -> Result<()> {
    // Resolve column titles against the card's current board.
    let column = if super::resolve::looks_like_id(column) {
        column.to_string()
    } else {
        let card = client
            .cards()
            .get_card_with_options(
                v1::GetCardRequest {
                    card_id: card_id.to_string(),
                    ..Default::default()
                },
                object_id_options(card_id),
            )
            .await?
            .into_owned();
        let board_id = required(card.card, "card")?.board_id;
        super::resolve::NameResolver::new(client)
            .column(&board_id, column)
            .await?
    };
    let resp = client
        .cards()
        .move_card_with_options(
            v1::MoveCardRequest {
                card_id: card_id.to_string(),
                to_column_id: column,
                to_position: position.unwrap_or_default(),
                idempotency_key: new_idempotency_key(),
                ..Default::default()
            },
            object_id_options(card_id),
        )
        .await?
        .into_owned();
    render_card_detail(required(resp.card, "card")?, format).await
}

/// Relocate a card to another board: recreate-and-close.
///
/// The server rejects cross-project moves, so the card is recreated on the
/// target board (title, description, priority, urgency, due, labels,
/// assignees, and checklist copied), both cards are cross-referenced with
/// comments, and the source is then deleted — never moved to a done column,
/// which would stamp `completed_at` and pollute completion metrics. Any
/// failure before the delete leaves the source card untouched.
async fn move_card_to_board(
    client: &KanbanClient,
    card_id: &str,
    board_id: &str,
    column: &str,
    position: Option<i32>,
    format: OutputFormat,
) -> Result<()> {
    let source = required(
        client
            .cards()
            .get_card_with_options(
                v1::GetCardRequest {
                    card_id: card_id.to_string(),
                    ..Default::default()
                },
                object_id_options(card_id),
            )
            .await?
            .into_owned()
            .card,
        "card",
    )?;

    // A --board naming the card's own board is a plain column move.
    if source.board_id == board_id {
        return move_card_within_board(client, card_id, column, position, format).await;
    }

    let resolver = super::resolve::NameResolver::new(client);
    let column_id = resolver.column(board_id, column).await?;

    // Summarize what deleting the source would lose (no copy endpoints exist
    // for comments or attachments) so the origin comment can carry it over.
    let comments = if source.comments_count > 0 {
        client
            .cards()
            .list_comments_with_options(
                v1::ListCommentsRequest {
                    card_id: card_id.to_string(),
                    ..Default::default()
                },
                object_id_options(card_id),
            )
            .await?
            .into_owned()
            .comments
    } else {
        Vec::new()
    };
    let attachments = if source.attachments_count > 0 {
        client
            .attachments()
            .list_attachments_by_card_with_options(
                v1::ListAttachmentsByCardRequest {
                    card_id: card_id.to_string(),
                    ..Default::default()
                },
                object_id_options(card_id),
            )
            .await?
            .into_owned()
            .attachments
    } else {
        Vec::new()
    };

    // Copy the card itself. The milestone is project-scoped, so it is not
    // carried to a board in another project.
    let mut new_card = required(
        client
            .cards()
            .create_card_with_options(
                v1::CreateCardRequest {
                    board_id: board_id.to_string(),
                    column_id,
                    title: source.title.clone(),
                    description: source.description.clone(),
                    priority: source.priority,
                    due: source.due.clone(),
                    milestone_id: String::new(),
                    position: position.unwrap_or_default(),
                    idempotency_key: new_idempotency_key(),
                    urgency: source.urgency,
                    ..Default::default()
                },
                object_id_options(board_id),
            )
            .await?
            .into_owned()
            .card,
        "card",
    )?;

    // Labels are project-scoped: re-match the source label names against the
    // target project's catalog and push the surviving IDs wholesale.
    let mut dropped_labels: Vec<String> = Vec::new();
    if !source.labels.is_empty() {
        let catalog = resolver.project_labels(&new_card.project_id).await?;
        let mut label_ids = Vec::new();
        for label in &source.labels {
            match catalog
                .iter()
                .find(|c| super::resolve::name_matches(&c.name, &label.name))
            {
                Some(c) => label_ids.push(c.id.clone()),
                None => dropped_labels.push(label.name.clone()),
            }
        }
        if !label_ids.is_empty() {
            let resp = client
                .cards()
                .bulk_update_card_labels_with_options(
                    v1::BulkUpdateCardLabelsRequest {
                        card_ids: vec![new_card.id.clone()],
                        label_ids,
                        idempotency_key: new_idempotency_key(),
                        ..Default::default()
                    },
                    object_id_options(&new_card.id),
                )
                .await?
                .into_owned();
            if let Some(card) = resp.cards.into_iter().next() {
                new_card = card;
            }
        }
    }

    // Assignee subjects are global identities and copy across projects.
    for assignee in &source.assignees {
        if assignee.subject.is_empty() {
            continue;
        }
        let resp = client
            .cards()
            .assign_card_with_options(
                v1::AssignCardRequest {
                    card_id: new_card.id.clone(),
                    subject: assignee.subject.clone(),
                    ..Default::default()
                },
                mutating_options(&new_card.id),
            )
            .await?
            .into_owned();
        if let Some(card) = resp.card.into_option() {
            new_card = card;
        }
    }

    // CreateCard takes no checklist; re-add each item in order and restore
    // the done flag via a follow-up patch.
    for item in &source.checklist {
        let resp = client
            .cards()
            .add_checklist_item_with_options(
                v1::AddChecklistItemRequest {
                    card_id: new_card.id.clone(),
                    text: item.text.clone(),
                    position: item.position,
                    idempotency_key: new_idempotency_key(),
                    ..Default::default()
                },
                object_id_options(&new_card.id),
            )
            .await?
            .into_owned();
        if let Some(card) = resp.card.into_option() {
            new_card = card;
        }
        if item.done
            && let Some(added) = new_card
                .checklist
                .iter()
                .find(|c| c.text == item.text && c.position == item.position)
                .map(|c| c.id.clone())
        {
            let resp = client
                .cards()
                .update_checklist_item_with_options(
                    v1::UpdateChecklistItemRequest {
                        card_id: new_card.id.clone(),
                        item_id: added,
                        item: v1::ChecklistItem {
                            done: true,
                            ..Default::default()
                        }
                        .into(),
                        update_mask: Some(buffa_types::google::protobuf::FieldMask {
                            paths: vec!["done".to_string()],
                            ..Default::default()
                        })
                        .into(),
                        ..Default::default()
                    },
                    mutating_options(&new_card.id),
                )
                .await?
                .into_owned();
            if let Some(card) = resp.card.into_option() {
                new_card = card;
            }
        }
    }

    // Cross-reference both directions.
    client
        .cards()
        .add_comment_with_options(
            v1::AddCommentRequest {
                card_id: new_card.id.clone(),
                body: origin_note(&source, &comments, &attachments, &dropped_labels),
                idempotency_key: new_idempotency_key(),
                ..Default::default()
            },
            object_id_options(&new_card.id),
        )
        .await?;
    client
        .cards()
        .add_comment_with_options(
            v1::AddCommentRequest {
                card_id: card_id.to_string(),
                body: format!(
                    "Moved to {} via `sunbeam kanban card move`; this card is closed by deletion.",
                    new_card.r#ref
                ),
                idempotency_key: new_idempotency_key(),
                ..Default::default()
            },
            object_id_options(card_id),
        )
        .await?;

    // Close the source without stamping completed_at.
    client
        .cards()
        .delete_card_with_options(
            v1::DeleteCardRequest {
                card_id: card_id.to_string(),
                ..Default::default()
            },
            object_id_options(card_id),
        )
        .await?;
    tracing::info!(
        "kanban card moved across boards: {} deleted after recreation as {}",
        source.r#ref,
        new_card.r#ref
    );
    render_card_detail(new_card, format).await
}

/// Body of the origin comment left on the recreated card.
///
/// Deleting the source card drops its comments, attachments, and GitHub
/// links (no copy endpoints exist), so they are summarized here, along with
/// any labels that have no namesake on the target project.
fn origin_note(
    source: &v1::Card,
    comments: &[v1::Comment],
    attachments: &[v1::Attachment],
    dropped_labels: &[String],
) -> String {
    let mut note = format!(
        "Moved from {} via `sunbeam kanban card move`.",
        source.r#ref
    );
    let mut losses: Vec<String> = Vec::new();
    if !comments.is_empty() {
        let recent: Vec<String> = comments
            .iter()
            .rev()
            .take(3)
            .map(|c| {
                format!(
                    "\"{}\"",
                    truncate(c.body.lines().next().unwrap_or_default(), 120)
                )
            })
            .collect();
        losses.push(format!(
            "{} comment(s); most recent: {}",
            comments.len(),
            recent.join(", ")
        ));
    }
    if !attachments.is_empty() {
        losses.push(format!(
            "{} attachment(s): {}",
            attachments.len(),
            attachments
                .iter()
                .map(|a| a.filename.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !source.github_links.is_empty() {
        losses.push(format!(
            "GitHub links: {}",
            source
                .github_links
                .iter()
                .map(|g| format!("{}#{} ({})", g.repo, g.number, g.state))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !dropped_labels.is_empty() {
        losses.push(format!(
            "labels not on the target project: {}",
            dropped_labels.join(", ")
        ));
    }
    note.push_str("\n\nThe source card was deleted after the copy");
    if losses.is_empty() {
        note.push('.');
    } else {
        note.push_str(&format!(
            "; not carried over:\n{}",
            losses
                .iter()
                .map(|l| format!("- {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    note
}

/// Run a card command.
pub(crate) async fn run(
    cmd: CardAction,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        CardAction::List { board, column } => {
            let column = match column {
                Some(raw) => Some(
                    super::resolve::NameResolver::new(client)
                        .column(&board, &raw)
                        .await?,
                ),
                None => None,
            };
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
                    "REF",
                    "COLUMN",
                    "TITLE",
                    "PRIORITY",
                    "BLOCKED",
                    "ASSIGNEES",
                    "POSITION",
                    "CREATED",
                    "ID",
                ],
                |c| {
                    vec![
                        c.r#ref.clone(),
                        c.column_id.clone(),
                        c.title.clone(),
                        c.priority.clone(),
                        c.blocked.to_string(),
                        c.assignees.join(", "),
                        c.position.to_string(),
                        c.created_at.clone(),
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
            urgency,
        } => {
            let column = super::resolve::NameResolver::new(client)
                .column_for_create(&board, column.as_deref())
                .await?;
            let resp = client
                .cards()
                .create_card_with_options(
                    v1::CreateCardRequest {
                        board_id: board.clone(),
                        column_id: column,
                        title,
                        description: description.unwrap_or_default(),
                        priority: priority
                            .map(v1::CardPriority::from)
                            .unwrap_or(v1::CardPriority::Unspecified)
                            .into(),
                        milestone_id: String::new(),
                        position: 0,
                        idempotency_key: new_idempotency_key(),
                        urgency: urgency
                            .map(v1::CardUrgency::from)
                            .unwrap_or(v1::CardUrgency::Medium)
                            .into(),
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
            urgency,
            blocked,
            unblocked,
            milestone,
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
            if let Some(u) = urgency {
                update_card.urgency = v1::CardUrgency::from(u).into();
                paths.push("urgency".to_string());
            }
            if blocked {
                update_card.blocked = true;
                paths.push("blocked".to_string());
            } else if unblocked {
                update_card.blocked = false;
                paths.push("blocked".to_string());
            }
            if let Some(m) = milestone {
                update_card.milestone_id = resolve_milestone_arg(client, &card_id, &m).await?;
                paths.push("milestone_id".to_string());
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
            board,
        } => match board {
            Some(board) => {
                move_card_to_board(client, &card_id, &board, &column, position, format).await
            }
            None => move_card_within_board(client, &card_id, &column, position, format).await,
        },
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
        CardAction::Assign { card_id, subject } => {
            let subject = resolve_subject(&subject).await?;
            let resp = client
                .cards()
                .assign_card_with_options(
                    v1::AssignCardRequest {
                        card_id: card_id.clone(),
                        subject,
                        ..Default::default()
                    },
                    mutating_options(&card_id),
                )
                .await?
                .into_owned();
            render_card_detail(required(resp.card, "card")?, format).await
        }
        CardAction::Unassign { card_id, subject } => {
            let subject = resolve_subject(&subject).await?;
            let resp = client
                .cards()
                .unassign_card_with_options(
                    v1::UnassignCardRequest {
                        card_id: card_id.clone(),
                        subject,
                        ..Default::default()
                    },
                    mutating_options(&card_id),
                )
                .await?
                .into_owned();
            render_card_detail(required(resp.card, "card")?, format).await
        }
        CardAction::Label { action } => {
            let (card_id, names, mode) = match action {
                CardLabelAction::Set { card_id, names } => (card_id, names, LabelMode::Set),
                CardLabelAction::Add { card_id, names } => (card_id, names, LabelMode::Add),
                CardLabelAction::Remove { card_id, names } => (card_id, names, LabelMode::Remove),
            };
            update_card_labels(client, &card_id, &names, mode, format).await
        }
        CardAction::Comment { action } => match action {
            CommentAction::List { card_id } => {
                let resp = client
                    .cards()
                    .list_comments_with_options(
                        v1::ListCommentsRequest {
                            card_id: card_id.clone(),
                            ..Default::default()
                        },
                        object_id_options(&card_id),
                    )
                    .await?
                    .into_owned();
                let comments: Vec<CommentOut> = resp.comments.into_iter().map(Into::into).collect();
                render_list(
                    &comments,
                    &["ID", "AUTHOR", "BODY", "CREATED"],
                    |c| {
                        vec![
                            c.id.clone(),
                            c.author_sub.clone(),
                            c.body.clone(),
                            c.created_at.clone(),
                        ]
                    },
                    format,
                )
            }
            CommentAction::Add { card_id, message } => {
                let resp = client
                    .cards()
                    .add_comment_with_options(
                        v1::AddCommentRequest {
                            card_id: card_id.clone(),
                            body: message,
                            idempotency_key: new_idempotency_key(),
                            ..Default::default()
                        },
                        object_id_options(&card_id),
                    )
                    .await?
                    .into_owned();
                render(
                    &CommentOut::from(required(resp.comment, "comment")?),
                    format,
                )
            }
            CommentAction::Edit {
                card_id,
                comment_id,
                message,
            } => {
                let resp = client
                    .cards()
                    .edit_comment_with_options(
                        v1::EditCommentRequest {
                            card_id: card_id.clone(),
                            comment_id,
                            body: message,
                            ..Default::default()
                        },
                        mutating_options(&card_id),
                    )
                    .await?
                    .into_owned();
                render(
                    &CommentOut::from(required(resp.comment, "comment")?),
                    format,
                )
            }
            CommentAction::Delete {
                card_id,
                comment_id,
            } => {
                client
                    .cards()
                    .delete_comment_with_options(
                        v1::DeleteCommentRequest {
                            card_id: card_id.clone(),
                            comment_id,
                            ..Default::default()
                        },
                        mutating_options(&card_id),
                    )
                    .await?;
                Ok(())
            }
        },
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
    use wiremock::matchers::{header, method, path};
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
                urgency: Some(UrgencyArg::Critical),
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
                urgency: Some(UrgencyArg::High),
                blocked: true,
                unblocked: false,
                milestone: None,
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
                board: None,
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

        // Priority/urgency flags land on the wire (CLI-026).
        use sdk::kanban::prelude::buffa::Message;
        let requests = server.received_requests().await.unwrap();
        let create = requests
            .iter()
            .find(|r| r.url.path().ends_with("/CreateCard"))
            .expect("CreateCard request");
        let created = v1::CreateCardRequest::decode(&mut create.body.as_slice()).unwrap();
        assert_eq!(created.priority, v1::CardPriority::High);
        assert_eq!(created.urgency, v1::CardUrgency::Critical);
        let update = requests
            .iter()
            .find(|r| r.url.path().ends_with("/UpdateCard"))
            .expect("UpdateCard request");
        let updated = v1::UpdateCardRequest::decode(&mut update.body.as_slice()).unwrap();
        let card = updated.card.as_option().expect("update card patch");
        assert_eq!(card.priority, v1::CardPriority::Low);
        assert_eq!(card.urgency, v1::CardUrgency::High);
        let mask = updated.update_mask.as_option().expect("update mask");
        assert!(mask.paths.contains(&"urgency".to_string()));
    }

    #[tokio::test]
    async fn comment_list_add_edit_delete() {
        let server = MockServer::start().await;
        let comment = || v1::Comment {
            id: "cmt_1".into(),
            card_id: "card_1".into(),
            author_sub: "user:alice".into(),
            body: "looking into it".into(),
            ..Default::default()
        };
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/ListComments"))
            .respond_with(testutil::proto_response(&v1::ListCommentsResponse {
                comments: vec![comment()],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/AddComment"))
            .respond_with(testutil::proto_response(&v1::AddCommentResponse {
                comment: comment().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/EditComment"))
            .respond_with(testutil::proto_response(&v1::EditCommentResponse {
                comment: comment().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/DeleteComment"))
            .respond_with(testutil::proto_response(
                &v1::DeleteCommentResponse::default(),
            ))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Comment {
                action: CommentAction::List {
                    card_id: "card_1".into(),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Comment {
                action: CommentAction::Add {
                    card_id: "card_1".into(),
                    message: "looking into it".into(),
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Comment {
                action: CommentAction::Edit {
                    card_id: "card_1".into(),
                    comment_id: "cmt_1".into(),
                    message: "root cause found".into(),
                },
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Comment {
                action: CommentAction::Delete {
                    card_id: "card_1".into(),
                    comment_id: "cmt_1".into(),
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }

    const ULID: &str = "01HZY9JTKKHK3Y6XJJYHZ9Q5TV";

    /// Redirect HOME to a tempdir, point the sso-gateway client at the
    /// wiremock server, and seed a valid cached token.
    fn sso_env(server_uri: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var(crate::auth::SSO_URL_ENV, server_uri);
        }
        sdk::config::set_active_context(sdk::config::Context {
            domain: "example.com".into(),
            ..Default::default()
        });
        sdk::config::set_auth_tokens(
            "example.com",
            &sdk::config::AuthTokens {
                access_token: "cached-access".into(),
                refresh_token: String::new(),
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(3600),
                id_token: None,
            },
        )
        .unwrap();
        dir
    }

    /// Mount a GetIdentity responder for the sso-gateway IdentityService.
    async fn mount_identity(server: &MockServer) {
        use sdk::auth::v1 as iam;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::proto_response(&iam::Identity {
                id: ULID.into(),
                ..Default::default()
            }))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn assign_and_unassign() {
        let server = MockServer::start().await;
        mount_identity(&server).await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/AssignCard"))
            .respond_with(testutil::proto_response(&v1::AssignCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/UnassignCard"))
            .respond_with(testutil::proto_response(&v1::UnassignCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let _home = sso_env(&server.uri());
        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Assign {
                card_id: "card_1".into(),
                subject: ULID.into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Unassign {
                card_id: "card_1".into(),
                subject: format!("user:{ULID}"),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn assign_rejects_garbage_subject_locally() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/AssignCard"))
            .respond_with(testutil::proto_response(&v1::AssignCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .expect(0)
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            CardAction::Assign {
                card_id: "card_1".into(),
                subject: "tony".into(),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown user 'tony'"), "{msg}");
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

    fn labeled_card() -> v1::Card {
        v1::Card {
            labels: vec![v1::Label {
                id: "label_bug".into(),
                project_id: "proj_1".into(),
                name: "bug".into(),
                style: "red".into(),
                ..Default::default()
            }],
            ..lean_card("card_1", "Fix crash")
        }
    }

    /// Mount the GetCard + ListLabels + BulkUpdateCardLabels responders the
    /// label operations need.
    async fn mount_label_flow(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::proto_response(&v1::GetCardResponse {
                card: labeled_card().into(),
                ..Default::default()
            }))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/ListLabels"))
            .respond_with(testutil::proto_response(&v1::ListLabelsResponse {
                labels: vec![
                    v1::Label {
                        id: "label_bug".into(),
                        project_id: "proj_1".into(),
                        name: "bug".into(),
                        style: "red".into(),
                        ..Default::default()
                    },
                    v1::Label {
                        id: "label_backend".into(),
                        project_id: "proj_1".into(),
                        name: "backend".into(),
                        style: "blue".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/BulkUpdateCardLabels"))
            // Gated on KanbanBoard + edit: object id must be the board's.
            .and(header("x-sunbeam-object-id", "board_1"))
            .respond_with(testutil::proto_response(
                &v1::BulkUpdateCardLabelsResponse {
                    cards: vec![labeled_card()],
                    ..Default::default()
                },
            ))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn card_label_set_add_remove() {
        let server = MockServer::start().await;
        mount_label_flow(&server).await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Label {
                action: CardLabelAction::Set {
                    card_id: "card_1".into(),
                    names: vec!["backend".into()],
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Label {
                action: CardLabelAction::Add {
                    card_id: "card_1".into(),
                    names: vec!["BUG".into(), "label_backend".into()],
                },
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
        run(
            CardAction::Label {
                action: CardLabelAction::Remove {
                    card_id: "card_1".into(),
                    names: vec!["bug".into()],
                },
            },
            OutputFormat::Yaml,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn card_label_unknown_name_lists_available() {
        let server = MockServer::start().await;
        mount_label_flow(&server).await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            CardAction::Label {
                action: CardLabelAction::Add {
                    card_id: "card_1".into(),
                    names: vec!["nope".into()],
                },
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no label matches"), "{msg}");
        assert!(msg.contains("backend"), "{msg}");
    }

    #[tokio::test]
    async fn update_assigns_milestone_by_title() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::proto_response(&v1::GetCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.MilestoneService/ListMilestones"))
            .respond_with(testutil::proto_response(&v1::ListMilestonesResponse {
                milestones: vec![v1::Milestone {
                    id: "ms_1".into(),
                    project_id: "proj_1".into(),
                    title: "3.2".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/UpdateCard"))
            .respond_with(testutil::proto_response(&v1::UpdateCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Update {
                card_id: "card_1".into(),
                title: None,
                description: None,
                priority: None,
                urgency: None,
                blocked: false,
                unblocked: false,
                milestone: Some("3.2".into()),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn resolve_subject_rejects_non_id_subjects_locally() {
        for raw in ["tony", "user:tony", "user:", "auth0|abc123"] {
            let err = resolve_subject(raw).await.unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(&format!("unknown user '{raw}'")),
                "{raw}: {msg}"
            );
        }
    }

    #[tokio::test]
    async fn resolve_subject_verifies_ulid_via_identity_service() {
        let server = MockServer::start().await;
        mount_identity(&server).await;
        let _home = sso_env(&server.uri());

        assert_eq!(resolve_subject(ULID).await.unwrap(), format!("user:{ULID}"));
        assert_eq!(
            resolve_subject(&format!("user:{ULID}")).await.unwrap(),
            format!("user:{ULID}")
        );
    }

    #[tokio::test]
    async fn resolve_subject_errors_when_identity_missing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/iam.v1.IdentityService/GetIdentity"))
            .respond_with(testutil::connect_error(
                404,
                "not_found",
                "identity not found",
            ))
            .mount(&server)
            .await;
        let _home = sso_env(&server.uri());

        let err = resolve_subject(ULID).await.unwrap_err();
        assert!(err.to_string().contains("Identity not found"), "{err}");
    }

    #[test]
    fn card_out_surfaces_timestamps() {
        let ts = |seconds: i64| {
            buffa_types::google::protobuf::Timestamp {
                seconds,
                nanos: 0,
                ..Default::default()
            }
            .into()
        };
        let card = v1::Card {
            created_at: ts(1_700_000_000),
            updated_at: ts(1_700_100_000),
            completed_at: ts(1_700_200_000),
            ..lean_card("card_1", "Fix crash")
        };
        let out = CardOut::from(card);
        assert!(
            out.created_at.starts_with("2023-11-14T"),
            "{}",
            out.created_at
        );
        assert!(!out.updated_at.is_empty());
        assert!(!out.completed_at.is_empty());
        let open = CardOut::from(lean_card("card_2", "Open"));
        assert_eq!(open.created_at, "");
        assert_eq!(open.completed_at, "");
    }

    #[tokio::test]
    async fn update_unblocked_sends_false_patch() {
        use sdk::kanban::prelude::buffa::Message;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/UpdateCard"))
            .respond_with(testutil::proto_response(&v1::UpdateCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Update {
                card_id: "card_1".into(),
                title: None,
                description: None,
                priority: None,
                urgency: None,
                blocked: false,
                unblocked: true,
                milestone: None,
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let req = requests
            .iter()
            .find(|r| r.url.path().ends_with("/UpdateCard"))
            .expect("UpdateCard request");
        let decoded = v1::UpdateCardRequest::decode(&mut req.body.as_slice()).unwrap();
        let mask = decoded.update_mask.as_option().expect("update_mask");
        assert_eq!(mask.paths, vec!["blocked".to_string()]);
        let patch = decoded.card.as_option().expect("card patch");
        assert!(!patch.blocked);
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

    #[test]
    fn truncate_cuts_long_text_with_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        let long = "x".repeat(200);
        let out = truncate(&long, 120);
        assert_eq!(out.chars().count(), 121);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn origin_note_summarizes_losses() {
        let mut source = card("card_1", "Fix crash");
        let comments = vec![
            v1::Comment {
                id: "cmt_1".into(),
                body: "first".into(),
                ..Default::default()
            },
            v1::Comment {
                id: "cmt_2".into(),
                body: "second".into(),
                ..Default::default()
            },
        ];
        let attachments = vec![v1::Attachment {
            id: "att_1".into(),
            filename: "design.png".into(),
            ..Default::default()
        }];
        let note = origin_note(&source, &comments, &attachments, &["infra".to_string()]);
        assert!(note.contains("Moved from BEAM-1"), "{note}");
        assert!(note.contains("2 comment(s)"), "{note}");
        assert!(note.contains("\"second\""), "{note}");
        assert!(note.contains("1 attachment(s): design.png"), "{note}");
        assert!(note.contains("sunbeam/cli#42 (open)"), "{note}");
        assert!(
            note.contains("labels not on the target project: infra"),
            "{note}"
        );

        source.github_links.clear();
        let clean = origin_note(&source, &[], &[], &[]);
        assert!(clean.ends_with("deleted after the copy."), "{clean}");
        assert!(!clean.contains("not carried over"), "{clean}");
    }

    /// Source card for cross-board move tests: labels, an assignee, a done
    /// checklist item, a GitHub link, comments, and an attachment.
    fn move_source_card() -> v1::Card {
        let mut c = card("card_1", "Fix crash");
        c.description = "details".into();
        c.comments_count = 2;
        c.attachments_count = 1;
        c.due = buffa_types::google::protobuf::Timestamp {
            seconds: 1_700_000_000,
            nanos: 0,
            ..Default::default()
        }
        .into();
        c
    }

    /// The recreated card on the target board (no assignees, so rendering it
    /// never triggers sso-gateway email resolution).
    fn move_target_card() -> v1::Card {
        v1::Card {
            id: "card_9".into(),
            project_id: "proj_2".into(),
            board_id: "board_2".into(),
            column_id: "col_9".into(),
            r#ref: "TRI-4".into(),
            title: "Fix crash".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cross_board_move_recreates_and_deletes_source() {
        use sdk::kanban::prelude::buffa::Message;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::proto_response(&v1::GetCardResponse {
                card: move_source_card().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse {
                detail: v1::BoardDetail {
                    board: v1::Board {
                        id: "board_2".into(),
                        project_id: "proj_2".into(),
                        name: "Dev".into(),
                        ..Default::default()
                    }
                    .into(),
                    columns: vec![v1::Column {
                        id: "col_9".into(),
                        board_id: "board_2".into(),
                        title: "Todo".into(),
                        position: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/ListComments"))
            .respond_with(testutil::proto_response(&v1::ListCommentsResponse {
                comments: vec![
                    v1::Comment {
                        id: "cmt_1".into(),
                        body: "looking into it".into(),
                        ..Default::default()
                    },
                    v1::Comment {
                        id: "cmt_2".into(),
                        body: "root cause found".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.AttachmentService/ListAttachmentsByCard",
            ))
            .respond_with(testutil::proto_response(
                &v1::ListAttachmentsByCardResponse {
                    attachments: vec![v1::Attachment {
                        id: "att_1".into(),
                        filename: "design.png".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/CreateCard"))
            .respond_with(testutil::proto_response(&v1::CreateCardResponse {
                card: move_target_card().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.LabelService/ListLabels"))
            .respond_with(testutil::proto_response(&v1::ListLabelsResponse {
                labels: vec![v1::Label {
                    id: "label_bug2".into(),
                    project_id: "proj_2".into(),
                    name: "bug".into(),
                    style: "red".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/BulkUpdateCardLabels"))
            .respond_with(testutil::proto_response(
                &v1::BulkUpdateCardLabelsResponse {
                    cards: vec![move_target_card()],
                    ..Default::default()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/AssignCard"))
            .respond_with(testutil::proto_response(&v1::AssignCardResponse {
                card: move_target_card().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/AddChecklistItem"))
            .respond_with(testutil::proto_response(&v1::AddChecklistItemResponse {
                card: v1::Card {
                    checklist: vec![v1::ChecklistItem {
                        id: "chk_new".into(),
                        text: "repro".into(),
                        done: false,
                        position: 1,
                        ..Default::default()
                    }],
                    ..move_target_card()
                }
                .into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/UpdateChecklistItem"))
            .respond_with(testutil::proto_response(&v1::UpdateChecklistItemResponse {
                card: move_target_card().into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/AddComment"))
            .respond_with(testutil::proto_response(&v1::AddCommentResponse {
                comment: v1::Comment {
                    id: "cmt_new".into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/DeleteCard"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .expect(1)
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Move {
                card_id: "card_1".into(),
                column: "Todo".into(),
                position: None,
                board: Some("board_2".into()),
            },
            OutputFormat::Json,
            &client,
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let paths: Vec<&str> = requests.iter().map(|r| r.url.path()).collect();
        assert_eq!(
            paths,
            vec![
                "/sunbeam.kanban.v1.CardService/GetCard",
                "/sunbeam.kanban.v1.BoardService/GetBoard",
                "/sunbeam.kanban.v1.CardService/ListComments",
                "/sunbeam.kanban.v1.AttachmentService/ListAttachmentsByCard",
                "/sunbeam.kanban.v1.CardService/CreateCard",
                "/sunbeam.kanban.v1.LabelService/ListLabels",
                "/sunbeam.kanban.v1.CardService/BulkUpdateCardLabels",
                "/sunbeam.kanban.v1.CardService/AssignCard",
                "/sunbeam.kanban.v1.CardService/AddChecklistItem",
                "/sunbeam.kanban.v1.CardService/UpdateChecklistItem",
                "/sunbeam.kanban.v1.CardService/AddComment",
                "/sunbeam.kanban.v1.CardService/AddComment",
                "/sunbeam.kanban.v1.CardService/DeleteCard",
            ],
            "unexpected RPC sequence: {paths:?}"
        );

        // The copy carries the source card's fields.
        let create = requests
            .iter()
            .find(|r| r.url.path().ends_with("/CreateCard"))
            .expect("CreateCard request");
        let created = v1::CreateCardRequest::decode(&mut create.body.as_slice()).unwrap();
        assert_eq!(created.board_id, "board_2");
        assert_eq!(created.column_id, "col_9");
        assert_eq!(created.title, "Fix crash");
        assert_eq!(created.description, "details");
        assert_eq!(created.priority.to_i32(), 3);
        assert_eq!(created.urgency.to_i32(), 4);
        assert_eq!(
            created.due.as_option().map(|t| t.seconds),
            Some(1_700_000_000)
        );

        // Labels re-matched by name against the target project's catalog.
        let bulk = requests
            .iter()
            .find(|r| r.url.path().ends_with("/BulkUpdateCardLabels"))
            .expect("BulkUpdateCardLabels request");
        let bulked = v1::BulkUpdateCardLabelsRequest::decode(&mut bulk.body.as_slice()).unwrap();
        assert_eq!(bulked.card_ids, vec!["card_9".to_string()]);
        assert_eq!(bulked.label_ids, vec!["label_bug2".to_string()]);

        // Assignee subject copied.
        let assign = requests
            .iter()
            .find(|r| r.url.path().ends_with("/AssignCard"))
            .expect("AssignCard request");
        let assigned = v1::AssignCardRequest::decode(&mut assign.body.as_slice()).unwrap();
        assert_eq!(assigned.card_id, "card_9");
        assert_eq!(assigned.subject, "user:alice");

        // Cross-reference comments: origin on the new card first, then the
        // destination note on the source.
        let comments: Vec<v1::AddCommentRequest> = requests
            .iter()
            .filter(|r| r.url.path().ends_with("/AddComment"))
            .map(|r| v1::AddCommentRequest::decode(&mut r.body.as_slice()).unwrap())
            .collect();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].card_id, "card_9");
        assert!(comments[0].body.contains("Moved from BEAM-1"));
        assert!(comments[0].body.contains("design.png"));
        assert!(comments[0].body.contains("sunbeam/cli#42 (open)"));
        assert!(comments[0].body.contains("root cause found"));
        assert_eq!(comments[1].card_id, "card_1");
        assert!(comments[1].body.contains("Moved to TRI-4"));
    }

    #[tokio::test]
    async fn cross_board_move_failure_before_delete_leaves_source_untouched() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::proto_response(&v1::GetCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/GetBoard"))
            .respond_with(testutil::proto_response(&v1::GetBoardResponse {
                detail: v1::BoardDetail {
                    columns: vec![v1::Column {
                        id: "col_9".into(),
                        board_id: "board_2".into(),
                        title: "Todo".into(),
                        position: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/CreateCard"))
            .respond_with(testutil::connect_error(500, "internal", "db down"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/DeleteCard"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .expect(0)
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let err = run(
            CardAction::Move {
                card_id: "card_1".into(),
                column: "Todo".into(),
                position: None,
                board: Some("board_2".into()),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("db down"), "{err}");
    }

    #[tokio::test]
    async fn move_with_own_board_falls_back_to_plain_move() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/GetCard"))
            .respond_with(testutil::proto_response(&v1::GetCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/MoveCard"))
            .respond_with(testutil::proto_response(&v1::MoveCardResponse {
                card: lean_card("card_1", "Fix crash").into(),
                ..Default::default()
            }))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/CreateCard"))
            .respond_with(testutil::proto_response(&v1::CreateCardResponse {
                card: lean_card("card_9", "Fix crash").into(),
                ..Default::default()
            }))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/DeleteCard"))
            .respond_with(testutil::proto_response(
                &buffa_types::google::protobuf::Empty::default(),
            ))
            .expect(0)
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        run(
            CardAction::Move {
                card_id: "card_1".into(),
                column: "col_2".into(),
                position: Some(2),
                board: Some("board_1".into()),
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap();
    }
}
