//! Kanban board management — projects, boards, cards, and more via ConnectRPC.

mod aggregated;
mod attachments;
mod auth;
mod boards;
mod card_templates;
mod cards;
mod github_links;
mod labels;
mod milestones;
mod projects;
mod public_boards;
mod resolve;
mod search;
mod subscribe;
mod templates;

use clap::Subcommand;
use sdk::error::{Result, SunbeamError};
use sdk::kanban::KanbanClient;
use sdk::kanban::prelude::{buffa, buffa_types, connectrpc, sunbeam_g2v};

use crate::output::OutputFormat;

/// Top-level `kanban` subcommands.
#[derive(Debug, Clone, Subcommand)]
#[command(name = "kanban")]
pub enum KanbanCommand {
    /// Project management.
    Project {
        /// Project subcommand to run.
        #[command(subcommand)]
        action: projects::ProjectAction,
    },
    /// Board management.
    Board {
        /// Board subcommand to run.
        #[command(subcommand)]
        action: boards::BoardAction,
    },
    /// Aggregated (meta) board management.
    Aggregate {
        /// Aggregated board subcommand to run.
        #[command(subcommand)]
        action: aggregated::AggregateAction,
    },
    /// Card management.
    Card {
        /// Card subcommand to run.
        #[command(subcommand)]
        action: cards::CardAction,
    },
    /// Label catalog management.
    Label {
        /// Label subcommand to run.
        #[command(subcommand)]
        action: labels::LabelAction,
    },
    /// Milestone management.
    Milestone {
        /// Milestone subcommand to run.
        #[command(subcommand)]
        action: milestones::MilestoneAction,
    },
    /// Board templates.
    Template {
        /// Template subcommand to run.
        #[command(subcommand)]
        action: templates::TemplateAction,
    },
    /// Card templates.
    #[command(name = "card-template")]
    CardTemplate {
        /// Card template subcommand to run.
        #[command(subcommand)]
        action: card_templates::CardTemplateAction,
    },
    /// Attachment management.
    Attachment {
        /// Attachment subcommand to run.
        #[command(subcommand)]
        action: attachments::AttachmentAction,
    },
    /// GitHub issue/PR links.
    #[command(name = "github")]
    GitHub {
        /// GitHub link subcommand to run.
        #[command(subcommand)]
        action: github_links::GitHubAction,
    },
    /// Full-text card search.
    Search(search::SearchArgs),
    /// Realtime board/project event subscriptions.
    Subscribe {
        /// Subscription subcommand to run.
        #[command(subcommand)]
        action: subscribe::SubscribeAction,
    },
    /// Public board read access (unauthenticated).
    #[command(name = "public-board")]
    PublicBoard {
        /// Public board subcommand to run.
        #[command(subcommand)]
        action: public_boards::PublicBoardAction,
    },
    /// Kanban session helpers (whoami, logout).
    Auth {
        /// Auth subcommand to run.
        #[command(subcommand)]
        action: auth::AuthAction,
    },
}

/// Generate a fresh ULID idempotency key for mutating RPCs.
pub(crate) fn new_idempotency_key() -> String {
    ulid::Ulid::new().to_string()
}

/// Resolve the default Kanban server URL for a context domain.
pub(crate) fn default_server_url_for(domain: &str) -> Result<String> {
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "no domain configured; set one with `sunbeam config set --domain ...` or pass --url",
        ));
    }
    Ok(format!("https://kanban.{domain}"))
}

/// Resolve the final server URL from an explicit override or the active context.
pub(crate) fn resolve_server_url(url_override: Option<&str>) -> Result<String> {
    match url_override {
        Some(u) => Ok(u.to_string()),
        None => default_server_url_for(&sdk::config::active_context().domain),
    }
}

/// Resolve and validate a bearer token for authenticated RPCs.
async fn require_token() -> Result<String> {
    // Only genuine auth failures get the re-login hint — transport errors
    // already carry their own connectivity context.
    crate::auth::get_token().await.map_err(|e| match e {
        SunbeamError::Identity(msg) => {
            SunbeamError::identity(format!("run `sunbeam auth login` first: {msg}"))
        }
        other => other,
    })
}

/// Build a [`KanbanClient`] for `server`.
///
/// When `token` is present the client injects `Authorization: Bearer <token>`
/// on every request; public-board commands pass `None`.
pub(crate) fn build_client(server: &str, token: Option<&str>) -> Result<KanbanClient> {
    let Some(token) = token else {
        return KanbanClient::connect(server)
            .map_err(|e| SunbeamError::network(format!("failed to build kanban client: {e}")));
    };
    let g2v = KanbanClient::builder(server)
        .auth(sunbeam_g2v::client::BearerToken::new(token))
        .build()
        .map_err(|e| SunbeamError::network(format!("failed to build kanban client: {e}")))?;
    let uri = server
        .parse()
        .map_err(|e| SunbeamError::config(format!("invalid kanban server URL {server}: {e}")))?;
    KanbanClient::new(g2v, uri)
        .map_err(|e| SunbeamError::network(format!("failed to build kanban client: {e}")))
}

/// Per-call options carrying the `x-sunbeam-object-id` header the server's
/// keto_dispatch middleware requires on object-scoped RPCs.
pub(crate) fn object_id_options(object_id: &str) -> connectrpc::client::CallOptions {
    connectrpc::client::CallOptions::default().with_header("x-sunbeam-object-id", object_id)
}

/// Like [`object_id_options`], plus a fresh `x-sunbeam-idempotency-key` header
/// for mutating RPCs whose request body carries no idempotency field.
pub(crate) fn mutating_options(object_id: &str) -> connectrpc::client::CallOptions {
    object_id_options(object_id).with_header("x-sunbeam-idempotency-key", new_idempotency_key())
}

/// One parsed `--columns` entry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColumnSpec {
    pub title: String,
    pub accent: String,
    pub is_done: bool,
    pub position: i32,
}

/// Parse a `--columns` spec: comma-separated `title[:accent][!]` entries
/// (position by order, `!` marks a completion lane). Shared by
/// `template create/update --columns` and `board create --columns` (CLI-024).
pub(crate) fn parse_columns_spec(spec: &str) -> Result<Vec<ColumnSpec>> {
    let mut out = Vec::new();
    for entry in spec.split(',').map(str::trim) {
        if entry.is_empty() {
            continue;
        }
        let (entry, is_done) = match entry.strip_suffix('!') {
            Some(stripped) => (stripped.trim_end(), true),
            None => (entry, false),
        };
        let (title, accent) = match entry.split_once(':') {
            Some((t, a)) => (t.trim(), a.trim()),
            None => (entry, ""),
        };
        if title.is_empty() {
            sdk::bail!("empty column title in --columns entry {entry:?}");
        }
        out.push(ColumnSpec {
            title: title.to_string(),
            accent: accent.to_string(),
            is_done,
            position: out.len() as i32 + 1,
        });
    }
    if out.is_empty() {
        sdk::bail!("--columns needs at least one column title");
    }
    Ok(out)
}

/// Unwrap a required message field from a synthesized RPC response wrapper.
///
/// The ConnectRPC codegen wraps bare `returns (Message)` responses in a
/// per-RPC struct holding a `MessageField`; the server always populates it.
pub(crate) fn required<T: Default>(field: buffa::MessageField<T>, what: &str) -> Result<T> {
    field
        .into_option()
        .ok_or_else(|| SunbeamError::network(format!("server response missing {what}")))
}

/// Format a protobuf Timestamp message field as RFC 3339 (empty when unset).
pub(crate) fn fmt_ts(ts: &buffa::MessageField<buffa_types::google::protobuf::Timestamp>) -> String {
    ts.as_option()
        .and_then(|t| chrono::DateTime::from_timestamp(t.seconds, t.nanos.max(0) as u32))
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// Returns true for transport-class failures worth one cold-start retry.
///
/// Fresh H2 connection setup intermittently fails the first RPC of a new
/// process (a Connect `unavailable` whose message is the reqwest send
/// failure); server-side application errors are never retried. The
/// connectrpc crate maps transport failures to `ErrorCode::Unavailable`
/// with no structured transport marker, so the reqwest send-failure
/// substring is still the only way to tell them apart from a server-side
/// `unavailable`.
fn is_cold_start_transport(err: &SunbeamError) -> bool {
    match err {
        SunbeamError::Connect { code, context } => {
            *code == connectrpc::ErrorCode::Unavailable && context.contains("error sending request")
        }
        _ => false,
    }
}

/// Server rejected the call as unauthenticated (distinct from permission
/// denials, which a fresh token would not fix).
fn is_unauthenticated(err: &SunbeamError) -> bool {
    matches!(
        err,
        SunbeamError::Connect {
            code: connectrpc::ErrorCode::Unauthenticated,
            ..
        }
    )
}

/// Run `op`, retrying once on a cold-start transport failure.
///
/// Reads are naturally idempotent and mutations carry ULID idempotency keys,
/// so a single retry is safe for every subcommand. Each attempt runs `op`
/// fresh so callers can rebuild their client per attempt.
async fn retry_once_on_transport<F, Fut>(logger: &sdk::logger::Logger, op: F) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    match op().await {
        Err(e) if is_cold_start_transport(&e) => {
            sdk::info!(
                logger,
                "kanban transport error on first attempt, retrying once",
                err = e.to_string()
            );
            op().await
        }
        result => result,
    }
}

/// Dispatch a kanban subcommand.
pub async fn dispatch(
    logger: &sdk::logger::Logger,
    cmd: KanbanCommand,
    format: OutputFormat,
    url_override: Option<&str>,
) -> Result<()> {
    let server = resolve_server_url(url_override)?;
    sdk::info!(logger, "kanban dispatch", server = server.as_str());

    match cmd {
        KanbanCommand::Auth { action } => auth::run(action, format).await,
        KanbanCommand::PublicBoard { action } => {
            retry_once_on_transport(logger, || {
                dispatch_public_board(action.clone(), format, &server)
            })
            .await
        }
        cmd => {
            let token = require_token().await?;
            let run_once = |token: &str| {
                let cmd = cmd.clone();
                let token = token.to_string();
                let server = server.clone();
                async move {
                    retry_once_on_transport(logger, || {
                        let cmd = cmd.clone();
                        let token = token.clone();
                        let server = server.clone();
                        async move {
                            let client = build_client(&server, Some(&token))?;
                            dispatch_authed(cmd, format, &client).await
                        }
                    })
                    .await
                }
            };
            match run_once(&token).await {
                // The cached token passed the local expiry check but the
                // server rejected it (CLI-021): force a refresh and retry
                // once before surfacing the failure.
                Err(e) if is_unauthenticated(&e) => {
                    sdk::info!(
                        logger,
                        "kanban call unauthenticated; forcing token refresh and retrying once",
                        err = e.to_string()
                    );
                    let fresh = crate::auth::force_refresh_token().await?;
                    run_once(&fresh).await
                }
                result => result,
            }
        }
    }
}

/// Resolve a `--board` reference: `project/board`, a bare board name, or an ID.
async fn resolve_board_ref(resolver: &resolve::NameResolver<'_>, raw: &str) -> Result<String> {
    if let Some((project, board)) = raw.split_once('/') {
        let project_id = resolver.project(project).await?;
        return resolver.board(&project_id, board).await;
    }
    resolver.board_anywhere(raw).await
}

/// Dispatch an authenticated kanban subcommand against a ready client.
///
/// Split from [`dispatch`] so the name-resolution remapping can be exercised
/// against a mock server in tests.
async fn dispatch_authed(
    cmd: KanbanCommand,
    format: OutputFormat,
    client: &KanbanClient,
) -> Result<()> {
    match cmd {
        KanbanCommand::Project { action } => projects::run(action, format, client).await,
        KanbanCommand::Board { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                boards::BoardAction::List { project } => boards::BoardAction::List {
                    project: resolver.project(&project).await?,
                },
                boards::BoardAction::Get { board_id } => boards::BoardAction::Get {
                    board_id: resolver.board_anywhere(&board_id).await?,
                },
                boards::BoardAction::Create {
                    project,
                    name,
                    description,
                    icon,
                    visibility,
                    template,
                    columns,
                } => boards::BoardAction::Create {
                    project: resolver.project(&project).await?,
                    name,
                    description,
                    icon,
                    visibility,
                    template,
                    columns,
                },
                boards::BoardAction::Update {
                    board_id,
                    name,
                    description,
                    icon,
                    visibility,
                } => boards::BoardAction::Update {
                    board_id: resolver.board_anywhere(&board_id).await?,
                    name,
                    description,
                    icon,
                    visibility,
                },
                boards::BoardAction::Delete { board_id } => boards::BoardAction::Delete {
                    board_id: resolver.board_anywhere(&board_id).await?,
                },
                boards::BoardAction::Column { action } => {
                    let action = match action {
                        boards::ColumnAction::Add {
                            board_id,
                            title,
                            accent,
                            wip_limit,
                            position,
                            is_done,
                        } => boards::ColumnAction::Add {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            title,
                            accent,
                            wip_limit,
                            position,
                            is_done,
                        },
                        boards::ColumnAction::Update {
                            board_id,
                            column_id,
                            title,
                            accent,
                            wip_limit,
                            is_done,
                            no_is_done,
                        } => boards::ColumnAction::Update {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            column_id,
                            title,
                            accent,
                            wip_limit,
                            is_done,
                            no_is_done,
                        },
                        boards::ColumnAction::Remove {
                            board_id,
                            column_id,
                        } => boards::ColumnAction::Remove {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            column_id,
                        },
                        boards::ColumnAction::Move {
                            board_id,
                            column_id,
                            position,
                        } => boards::ColumnAction::Move {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            column_id,
                            position,
                        },
                    };
                    boards::BoardAction::Column { action }
                }
            };
            boards::run(action, format, client).await
        }
        KanbanCommand::Aggregate { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                aggregated::AggregateAction::Source { action } => {
                    let action = match action {
                        aggregated::SourceAction::Add {
                            aggregate_id,
                            board_id,
                            position,
                        } => aggregated::SourceAction::Add {
                            aggregate_id,
                            board_id: resolver.board_anywhere(&board_id).await?,
                            position,
                        },
                        aggregated::SourceAction::Remove {
                            aggregate_id,
                            board_id,
                        } => aggregated::SourceAction::Remove {
                            aggregate_id,
                            board_id: resolver.board_anywhere(&board_id).await?,
                        },
                        aggregated::SourceAction::Move {
                            aggregate_id,
                            board_id,
                            position,
                        } => aggregated::SourceAction::Move {
                            aggregate_id,
                            board_id: resolver.board_anywhere(&board_id).await?,
                            position,
                        },
                    };
                    aggregated::AggregateAction::Source { action }
                }
                other => other,
            };
            aggregated::run(action, format, client).await
        }
        KanbanCommand::Card { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                cards::CardAction::List { board, column } => cards::CardAction::List {
                    board: resolver.board_anywhere(&board).await?,
                    column,
                },
                cards::CardAction::Get { card_id } => cards::CardAction::Get {
                    card_id: resolver.card_anywhere(&card_id).await?,
                },
                cards::CardAction::Create {
                    board,
                    column,
                    title,
                    description,
                    priority,
                    urgency,
                } => cards::CardAction::Create {
                    board: resolver.board_anywhere(&board).await?,
                    column,
                    title,
                    description,
                    priority,
                    urgency,
                },
                cards::CardAction::Update {
                    card_id,
                    title,
                    description,
                    priority,
                    urgency,
                    blocked,
                    unblocked,
                    milestone,
                } => cards::CardAction::Update {
                    card_id: resolver.card_anywhere(&card_id).await?,
                    title,
                    description,
                    priority,
                    urgency,
                    blocked,
                    unblocked,
                    milestone,
                },
                cards::CardAction::Assign { card_id, subject } => cards::CardAction::Assign {
                    card_id: resolver.card_anywhere(&card_id).await?,
                    subject,
                },
                cards::CardAction::Unassign { card_id, subject } => cards::CardAction::Unassign {
                    card_id: resolver.card_anywhere(&card_id).await?,
                    subject,
                },
                cards::CardAction::Label { action } => {
                    let action = match action {
                        cards::CardLabelAction::Set { card_id, names } => {
                            cards::CardLabelAction::Set {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                names,
                            }
                        }
                        cards::CardLabelAction::Add { card_id, names } => {
                            cards::CardLabelAction::Add {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                names,
                            }
                        }
                        cards::CardLabelAction::Remove { card_id, names } => {
                            cards::CardLabelAction::Remove {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                names,
                            }
                        }
                    };
                    cards::CardAction::Label { action }
                }
                cards::CardAction::Checklist { action } => {
                    let action = match action {
                        cards::CardChecklistAction::Add { card_id, text } => {
                            cards::CardChecklistAction::Add {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                text,
                            }
                        }
                        cards::CardChecklistAction::Toggle { card_id, item } => {
                            cards::CardChecklistAction::Toggle {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                item,
                            }
                        }
                        cards::CardChecklistAction::Remove { card_id, item } => {
                            cards::CardChecklistAction::Remove {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                item,
                            }
                        }
                        cards::CardChecklistAction::Set { card_id, items } => {
                            cards::CardChecklistAction::Set {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                items,
                            }
                        }
                    };
                    cards::CardAction::Checklist { action }
                }
                cards::CardAction::Comment { action } => {
                    let action = match action {
                        cards::CommentAction::List { card_id } => cards::CommentAction::List {
                            card_id: resolver.card_anywhere(&card_id).await?,
                        },
                        cards::CommentAction::Add { card_id, message } => {
                            cards::CommentAction::Add {
                                card_id: resolver.card_anywhere(&card_id).await?,
                                message,
                            }
                        }
                        cards::CommentAction::Edit {
                            card_id,
                            comment_id,
                            message,
                        } => cards::CommentAction::Edit {
                            card_id: resolver.card_anywhere(&card_id).await?,
                            comment_id,
                            message,
                        },
                        cards::CommentAction::Delete {
                            card_id,
                            comment_id,
                        } => cards::CommentAction::Delete {
                            card_id: resolver.card_anywhere(&card_id).await?,
                            comment_id,
                        },
                    };
                    cards::CardAction::Comment { action }
                }
                cards::CardAction::Move {
                    card_id,
                    column,
                    position,
                    board,
                } => cards::CardAction::Move {
                    card_id: resolver.card_anywhere(&card_id).await?,
                    column,
                    position,
                    board: match board {
                        Some(raw) => Some(resolve_board_ref(&resolver, &raw).await?),
                        None => None,
                    },
                },
                cards::CardAction::Delete { card_id } => cards::CardAction::Delete {
                    card_id: resolver.card_anywhere(&card_id).await?,
                },
                cards::CardAction::Dependency { action } => {
                    let action = match action {
                        cards::DependencyAction::Add {
                            board,
                            card_id,
                            depends_on,
                        } => cards::DependencyAction::Add {
                            board: resolver.board_anywhere(&board).await?,
                            card_id: resolver.card_anywhere(&card_id).await?,
                            depends_on: resolver.card_anywhere(&depends_on).await?,
                        },
                        cards::DependencyAction::Remove {
                            board,
                            card_id,
                            depends_on,
                        } => cards::DependencyAction::Remove {
                            board: resolver.board_anywhere(&board).await?,
                            card_id: resolver.card_anywhere(&card_id).await?,
                            depends_on: resolver.card_anywhere(&depends_on).await?,
                        },
                    };
                    cards::CardAction::Dependency { action }
                }
            };
            cards::run(action, format, client).await
        }
        KanbanCommand::Label { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                labels::LabelAction::List { project } => labels::LabelAction::List {
                    project: resolver.project(&project).await?,
                },
                labels::LabelAction::Create {
                    project,
                    name,
                    style,
                } => labels::LabelAction::Create {
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    name,
                    style,
                },
                labels::LabelAction::Update {
                    label,
                    project,
                    name,
                    style,
                } => labels::LabelAction::Update {
                    label,
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    name,
                    style,
                },
                labels::LabelAction::Delete { label, project } => labels::LabelAction::Delete {
                    label,
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                },
            };
            labels::run(action, format, client).await
        }
        KanbanCommand::Milestone { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                milestones::MilestoneAction::List { project } => {
                    milestones::MilestoneAction::List {
                        project: resolver.project(&project).await?,
                    }
                }
                milestones::MilestoneAction::Get { milestone, project } => {
                    milestones::MilestoneAction::Get {
                        milestone,
                        project: match project {
                            Some(p) => Some(resolver.project(&p).await?),
                            None => None,
                        },
                    }
                }
                milestones::MilestoneAction::Create {
                    project,
                    title,
                    due,
                } => milestones::MilestoneAction::Create {
                    project: resolver.project(&project).await?,
                    title,
                    due,
                },
                milestones::MilestoneAction::Update {
                    milestone,
                    project,
                    title,
                    due,
                } => milestones::MilestoneAction::Update {
                    milestone,
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    title,
                    due,
                },
                milestones::MilestoneAction::Delete { milestone, project } => {
                    milestones::MilestoneAction::Delete {
                        milestone,
                        project: match project {
                            Some(p) => Some(resolver.project(&p).await?),
                            None => None,
                        },
                    }
                }
            };
            milestones::run(action, format, client).await
        }
        KanbanCommand::Template { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                templates::TemplateAction::List { project } => templates::TemplateAction::List {
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                },
                templates::TemplateAction::Create {
                    project,
                    name,
                    description,
                    columns,
                } => templates::TemplateAction::Create {
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    name,
                    description,
                    columns,
                },
                other => other,
            };
            templates::run(action, format, client).await
        }
        KanbanCommand::CardTemplate { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                card_templates::CardTemplateAction::List { project } => {
                    card_templates::CardTemplateAction::List {
                        project: match project {
                            Some(p) => Some(resolver.project(&p).await?),
                            None => None,
                        },
                    }
                }
                card_templates::CardTemplateAction::Create {
                    project,
                    name,
                    description,
                    title,
                    default_description,
                    label,
                    checklist,
                } => card_templates::CardTemplateAction::Create {
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    name,
                    description,
                    title,
                    default_description,
                    label,
                    checklist,
                },
                other => other,
            };
            card_templates::run(action, format, client).await
        }
        KanbanCommand::Subscribe { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                subscribe::SubscribeAction::Board { board_id } => {
                    subscribe::SubscribeAction::Board {
                        board_id: resolver.board_anywhere(&board_id).await?,
                    }
                }
                subscribe::SubscribeAction::Project { project_id } => {
                    subscribe::SubscribeAction::Project {
                        project_id: resolver.project(&project_id).await?,
                    }
                }
            };
            subscribe::run(action, client).await
        }
        KanbanCommand::Attachment { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                attachments::AttachmentAction::List { card_id } => {
                    attachments::AttachmentAction::List {
                        card_id: resolver.card_anywhere(&card_id).await?,
                    }
                }
                attachments::AttachmentAction::Upload { card_id, file } => {
                    attachments::AttachmentAction::Upload {
                        card_id: resolver.card_anywhere(&card_id).await?,
                        file,
                    }
                }
                attachments::AttachmentAction::Download {
                    card,
                    attachment_id,
                    path,
                } => attachments::AttachmentAction::Download {
                    card: resolver.card_anywhere(&card).await?,
                    attachment_id,
                    path,
                },
                attachments::AttachmentAction::Delete {
                    card,
                    attachment_id,
                } => attachments::AttachmentAction::Delete {
                    card: resolver.card_anywhere(&card).await?,
                    attachment_id,
                },
            };
            attachments::run(action, format, client).await
        }
        KanbanCommand::GitHub { action } => {
            let resolver = resolve::NameResolver::new(client);
            let action = match action {
                github_links::GitHubAction::Link { card_id, issue } => {
                    github_links::GitHubAction::Link {
                        card_id: resolver.card_anywhere(&card_id).await?,
                        issue,
                    }
                }
                github_links::GitHubAction::Unlink { card, link_id } => {
                    github_links::GitHubAction::Unlink {
                        card: resolver.card_anywhere(&card).await?,
                        link_id,
                    }
                }
                github_links::GitHubAction::List { card_id } => github_links::GitHubAction::List {
                    card_id: resolver.card_anywhere(&card_id).await?,
                },
                github_links::GitHubAction::Search { card, repo, query } => {
                    github_links::GitHubAction::Search {
                        card: resolver.card_anywhere(&card).await?,
                        repo,
                        query,
                    }
                }
                github_links::GitHubAction::Resync { card, link_id } => {
                    github_links::GitHubAction::Resync {
                        card: resolver.card_anywhere(&card).await?,
                        link_id,
                    }
                }
            };
            github_links::run(action, format, client).await
        }
        KanbanCommand::Search(args) => search::run(args, format, client).await,
        KanbanCommand::Auth { .. } | KanbanCommand::PublicBoard { .. } => Err(SunbeamError::Other(
            "internal: unauthenticated command reached authenticated dispatch".into(),
        )),
    }
}

/// Dispatch a public-board subcommand.
///
/// Public boards are unauthenticated; the token is only required when a name
/// (rather than an ID) must be resolved first.
async fn dispatch_public_board(
    action: public_boards::PublicBoardAction,
    format: OutputFormat,
    server: &str,
) -> Result<()> {
    let action = match action {
        public_boards::PublicBoardAction::Get { board_id } => {
            if resolve::looks_like_id(&board_id) {
                public_boards::PublicBoardAction::Get { board_id }
            } else {
                let token = require_token().await?;
                let auth_client = build_client(server, Some(&token))?;
                let resolver = resolve::NameResolver::new(&auth_client);
                public_boards::PublicBoardAction::Get {
                    board_id: resolver.public_board_anywhere(&board_id).await?,
                }
            }
        }
        public_boards::PublicBoardAction::List { project_id } => {
            if resolve::looks_like_id(&project_id) {
                public_boards::PublicBoardAction::List { project_id }
            } else {
                let token = require_token().await?;
                let auth_client = build_client(server, Some(&token))?;
                let resolver = resolve::NameResolver::new(&auth_client);
                public_boards::PublicBoardAction::List {
                    project_id: resolver.project(&project_id).await?,
                }
            }
        }
    };
    let client = build_client(server, None)?;
    public_boards::run(action, format, &client).await
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use sdk::kanban::prelude::buffa::Message;
    use wiremock::ResponseTemplate;

    /// Build a client pointing at a wiremock server.
    pub(crate) fn client_for(uri: &str) -> KanbanClient {
        build_client(uri, Some("test-token")).expect("build test client")
    }

    /// Encode an owned message as a Connect unary proto response.
    pub(crate) fn proto_response<M: Message>(msg: &M) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/proto")
            .set_body_bytes(msg.encode_to_vec())
    }

    /// A Connect-protocol JSON error response.
    pub(crate) fn connect_error(status: u16, code: &str, message: &str) -> ResponseTemplate {
        ResponseTemplate::new(status)
            .insert_header("content-type", "application/json")
            .set_body_json(serde_json::json!({"code": code, "message": message}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_server_url_uses_override() {
        assert_eq!(
            resolve_server_url(Some("http://local")).unwrap(),
            "http://local"
        );
    }

    #[test]
    fn resolve_server_url_falls_back_to_active_context() {
        sdk::config::set_active_context(sdk::config::Context::default());
        let err = resolve_server_url(None).unwrap_err();
        assert!(err.to_string().contains("no domain configured"));
    }

    #[test]
    fn default_server_url_for_builds_from_domain() {
        assert_eq!(
            default_server_url_for("sunbeam.test").unwrap(),
            "https://kanban.sunbeam.test"
        );
    }

    #[test]
    fn default_server_url_for_errors_when_domain_empty() {
        let err = default_server_url_for("").unwrap_err();
        assert!(err.to_string().contains("no domain configured"));
    }

    #[test]
    fn new_idempotency_key_is_ulid() {
        let key = new_idempotency_key();
        assert!(!key.is_empty());
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn build_client_rejects_invalid_url() {
        let err = build_client("not a valid url ::://", None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid kanban server URL")
                || msg.contains("failed to build kanban client"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn build_client_constructs_with_and_without_token() {
        build_client("http://127.0.0.1:1", Some("token")).unwrap();
        build_client("http://127.0.0.1:1", None).unwrap();
    }

    #[test]
    fn object_id_options_sets_header() {
        let opts = object_id_options("board_abc123");
        assert_eq!(
            opts.headers().get("x-sunbeam-object-id").unwrap(),
            "board_abc123"
        );
    }

    #[test]
    fn mutating_options_sets_both_headers() {
        let opts = mutating_options("card_1");
        assert_eq!(opts.headers().get("x-sunbeam-object-id").unwrap(), "card_1");
        assert!(opts.headers().get("x-sunbeam-idempotency-key").is_some());
    }

    #[test]
    fn parse_columns_spec_parses_accents_done_and_positions() {
        let specs = parse_columns_spec("todo:blue, in progress,done:green!").unwrap();
        assert_eq!(
            specs,
            vec![
                ColumnSpec {
                    title: "todo".into(),
                    accent: "blue".into(),
                    is_done: false,
                    position: 1,
                },
                ColumnSpec {
                    title: "in progress".into(),
                    accent: String::new(),
                    is_done: false,
                    position: 2,
                },
                ColumnSpec {
                    title: "done".into(),
                    accent: "green".into(),
                    is_done: true,
                    position: 3,
                },
            ]
        );
    }

    #[test]
    fn parse_columns_spec_rejects_empty() {
        assert!(parse_columns_spec("  ").is_err());
        assert!(parse_columns_spec(":blue").is_err());
    }

    #[test]
    fn connect_error_converts_to_structured_variant_via_from() {
        let err = SunbeamError::from(connectrpc::ConnectError::new(
            connectrpc::ErrorCode::Unauthenticated,
            "bad token",
        ));
        assert!(matches!(
            err,
            SunbeamError::Connect {
                code: connectrpc::ErrorCode::Unauthenticated,
                ..
            }
        ));
        assert!(err.to_string().contains("bad token"));
    }

    #[test]
    fn is_unauthenticated_matches_only_connect_unauthenticated() {
        let unauth = SunbeamError::from(connectrpc::ConnectError::new(
            connectrpc::ErrorCode::Unauthenticated,
            "bad token",
        ));
        assert!(is_unauthenticated(&unauth));
        let denied = SunbeamError::from(connectrpc::ConnectError::new(
            connectrpc::ErrorCode::PermissionDenied,
            "nope",
        ));
        assert!(!is_unauthenticated(&denied));
        assert!(!is_unauthenticated(&SunbeamError::identity("expired")));
    }

    #[test]
    fn required_unwraps_and_errors() {
        let field: buffa::MessageField<String> = "x".to_string().into();
        assert_eq!(required(field, "project").unwrap(), "x");
        let empty: buffa::MessageField<String> = Default::default();
        let err = required(empty, "project").unwrap_err();
        assert!(err.to_string().contains("server response missing project"));
    }

    #[test]
    fn fmt_ts_formats_and_handles_unset() {
        let ts: buffa::MessageField<buffa_types::google::protobuf::Timestamp> =
            buffa_types::google::protobuf::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
                ..Default::default()
            }
            .into();
        assert!(fmt_ts(&ts).starts_with("2023-11-14T"));
        let unset: buffa::MessageField<buffa_types::google::protobuf::Timestamp> =
            Default::default();
        assert_eq!(fmt_ts(&unset), "");
    }

    async fn mount(server: &wiremock::MockServer, rpc: &str, body: wiremock::ResponseTemplate) {
        use wiremock::Mock;
        use wiremock::matchers::{method, path};

        Mock::given(method("POST"))
            .and(path(rpc.to_string()))
            .respond_with(body)
            .mount(server)
            .await;
    }

    /// An empty Connect streaming response body (end-of-stream frame only).
    fn empty_connect_stream() -> Vec<u8> {
        let end = b"{}";
        let mut body = vec![2u8]; // flags: end-of-stream
        body.extend_from_slice(&(end.len() as u32).to_be_bytes());
        body.extend_from_slice(end);
        body
    }

    /// Mount a broad set of kanban RPC mocks covering one entity of each kind.
    async fn full_server() -> wiremock::MockServer {
        use sdk::kanban::v1;
        use wiremock::MockServer;

        let server = MockServer::start().await;
        let project = v1::Project {
            id: "proj_1".into(),
            name: "Sunbeam".into(),
            prefix: "BEAM".into(),
            ..Default::default()
        };
        let board = v1::Board {
            id: "board_1".into(),
            project_id: "proj_1".into(),
            name: "Backlog".into(),
            ..Default::default()
        };
        let card = v1::Card {
            id: "card_1".into(),
            board_id: "board_1".into(),
            column_id: "col_1".into(),
            r#ref: "BEAM-1".into(),
            title: "Fix crash".into(),
            ..Default::default()
        };
        let agg = v1::AggregatedBoard {
            id: "agg_1".into(),
            name: "Meta".into(),
            ..Default::default()
        };

        mount(
            &server,
            "/sunbeam.kanban.v1.ProjectService/ListProjects",
            testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![project],
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.BoardService/ListBoards",
            testutil::proto_response(&v1::ListBoardsResponse {
                boards: vec![board.clone()],
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.BoardService/GetBoard",
            testutil::proto_response(&v1::GetBoardResponse {
                detail: v1::BoardDetail {
                    board: board.into(),
                    columns: vec![v1::Column {
                        id: "col_1".into(),
                        board_id: "board_1".into(),
                        title: "Todo".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.BoardService/DeleteBoard",
            testutil::proto_response(&buffa_types::google::protobuf::Empty::default()),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.CardService/GetCard",
            testutil::proto_response(&v1::GetCardResponse {
                card: card.clone().into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.CardService/AddCardDependency",
            testutil::proto_response(&v1::AddCardDependencyResponse {
                card: card.clone().into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.TemplatesService/GetTemplate",
            testutil::proto_response(&v1::GetTemplateResponse {
                template: v1::BoardTemplate {
                    id: "tmpl_1".into(),
                    name: "Standard".into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.CardService/CreateCard",
            testutil::proto_response(&v1::CreateCardResponse {
                card: card.into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.SearchService/SearchCards",
            testutil::proto_response(&v1::SearchCardsResponse {
                hits: vec![v1::CardSearchHit {
                    card_id: "card_1".into(),
                    card_ref: "BEAM-1".into(),
                    title: "Fix crash".into(),
                    board_id: "board_1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.AggregatedBoardService/ListAggregatedBoards",
            testutil::proto_response(&v1::ListAggregatedBoardsResponse {
                aggregated_boards: vec![agg.clone()],
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.AggregatedBoardService/AddSourceBoard",
            testutil::proto_response(&v1::AddSourceBoardResponse {
                aggregated_board: agg.into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.TemplatesService/ListTemplates",
            testutil::proto_response(&v1::ListTemplatesResponse::default()),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.TemplatesService/ListCardTemplates",
            testutil::proto_response(&v1::ListCardTemplatesResponse {
                templates: vec![v1::CardTemplate {
                    id: "ctmpl_1".into(),
                    name: "Bug".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.TemplatesService/GetCardTemplate",
            testutil::proto_response(&v1::GetCardTemplateResponse {
                template: v1::CardTemplate {
                    id: "ctmpl_1".into(),
                    name: "Bug".into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.BoardService/SubscribeBoard",
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/connect+proto")
                .set_body_bytes(empty_connect_stream()),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.ProjectService/SubscribeProject",
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/connect+proto")
                .set_body_bytes(empty_connect_stream()),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.AttachmentService/ListAttachmentsByCard",
            testutil::proto_response(&v1::ListAttachmentsByCardResponse::default()),
        )
        .await;
        mount(
            &server,
            "/sunbeam.kanban.v1.GithubLinkService/LinkIssue",
            testutil::proto_response(&v1::LinkIssueResponse {
                link: v1::GitHubLinkDetail {
                    id: "gh_1".into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }),
        )
        .await;
        server
    }

    #[tokio::test]
    async fn dispatch_authed_resolves_names_across_domains() {
        let server = full_server().await;
        let client = testutil::client_for(&server.uri());

        let cases = vec![
            KanbanCommand::Project {
                action: projects::ProjectAction::List,
            },
            KanbanCommand::Board {
                action: boards::BoardAction::List {
                    project: "sunbeam".into(),
                },
            },
            KanbanCommand::Board {
                action: boards::BoardAction::Get {
                    board_id: "backlog".into(),
                },
            },
            KanbanCommand::Board {
                action: boards::BoardAction::Delete {
                    board_id: "backlog".into(),
                },
            },
            KanbanCommand::Card {
                action: cards::CardAction::Create {
                    board: "backlog".into(),
                    column: None,
                    title: "New card".into(),
                    description: None,
                    priority: None,
                    urgency: None,
                },
            },
            KanbanCommand::Card {
                action: cards::CardAction::Get {
                    card_id: "BEAM-1".into(),
                },
            },
            KanbanCommand::Card {
                action: cards::CardAction::Dependency {
                    action: cards::DependencyAction::Add {
                        board: "backlog".into(),
                        card_id: "BEAM-1".into(),
                        depends_on: "BEAM-1".into(),
                    },
                },
            },
            KanbanCommand::Aggregate {
                action: aggregated::AggregateAction::Source {
                    action: aggregated::SourceAction::Add {
                        aggregate_id: "meta".into(),
                        board_id: "backlog".into(),
                        position: None,
                    },
                },
            },
            KanbanCommand::Template {
                action: templates::TemplateAction::List {
                    project: Some("sunbeam".into()),
                },
            },
            KanbanCommand::Template {
                action: templates::TemplateAction::Get {
                    template_id: "tmpl_1".into(),
                },
            },
            KanbanCommand::CardTemplate {
                action: card_templates::CardTemplateAction::List {
                    project: Some("sunbeam".into()),
                },
            },
            KanbanCommand::CardTemplate {
                action: card_templates::CardTemplateAction::Get {
                    project: None,
                    template_id: "ctmpl_1".into(),
                },
            },
            KanbanCommand::Subscribe {
                action: subscribe::SubscribeAction::Board {
                    board_id: "backlog".into(),
                },
            },
            KanbanCommand::Subscribe {
                action: subscribe::SubscribeAction::Project {
                    project_id: "sunbeam".into(),
                },
            },
            KanbanCommand::Attachment {
                action: attachments::AttachmentAction::List {
                    card_id: "BEAM-1".into(),
                },
            },
            KanbanCommand::GitHub {
                action: github_links::GitHubAction::Link {
                    card_id: "BEAM-1".into(),
                    issue: "sunbeam/cli#42".into(),
                },
            },
            KanbanCommand::Search(search::SearchArgs {
                query: "crash".into(),
                limit: None,
            }),
        ];

        for cmd in cases {
            dispatch_authed(cmd, OutputFormat::Table, &client)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn dispatch_authed_rejects_unauthenticated_commands() {
        let client = testutil::client_for("http://127.0.0.1:1");
        let err = dispatch_authed(
            KanbanCommand::Auth {
                action: auth::AuthAction::Whoami,
            },
            OutputFormat::Table,
            &client,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("unauthenticated command"));
    }

    #[tokio::test]
    async fn resolve_board_ref_handles_project_slash_board_and_bare_names() {
        use sdk::kanban::v1;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.ProjectService/ListProjects"))
            .respond_with(testutil::proto_response(&v1::ListProjectsResponse {
                projects: vec![v1::Project {
                    id: "proj_1".into(),
                    name: "Sunbeam".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.BoardService/ListBoards"))
            .respond_with(testutil::proto_response(&v1::ListBoardsResponse {
                boards: vec![v1::Board {
                    id: "board_9".into(),
                    project_id: "proj_1".into(),
                    name: "Dev".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let client = testutil::client_for(&server.uri());
        let resolver = resolve::NameResolver::new(&client);
        assert_eq!(
            resolve_board_ref(&resolver, "sunbeam/dev").await.unwrap(),
            "board_9"
        );
        assert_eq!(
            resolve_board_ref(&resolver, "dev").await.unwrap(),
            "board_9"
        );
        // ID-shaped input passes through without any RPC.
        let offline = testutil::client_for("http://127.0.0.1:1");
        let offline_resolver = resolve::NameResolver::new(&offline);
        assert_eq!(
            resolve_board_ref(&offline_resolver, "board_9")
                .await
                .unwrap(),
            "board_9"
        );
    }

    #[tokio::test]
    async fn dispatch_public_board_get_unauthenticated() {
        use sdk::kanban::v1;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.PublicBoardService/GetPublicBoard"))
            .respond_with(testutil::proto_response(&v1::GetPublicBoardResponse {
                board: v1::Board {
                    id: "board_1".into(),
                    name: "Roadmap".into(),
                    visibility: v1::BoardVisibility::Public.into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let logger = sdk::logger::Logger::new(sdk::logger::NoopSink);
        dispatch(
            &logger,
            KanbanCommand::PublicBoard {
                action: public_boards::PublicBoardAction::Get {
                    board_id: "board_1".into(),
                },
            },
            OutputFormat::Table,
            Some(&server.uri()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dispatch_public_board_list_unauthenticated() {
        use sdk::kanban::v1;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/sunbeam.kanban.v1.PublicBoardService/ListPublicBoards",
            ))
            .respond_with(testutil::proto_response(&v1::ListBoardsResponse {
                boards: vec![v1::Board {
                    id: "board_1".into(),
                    name: "Roadmap".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }))
            .mount(&server)
            .await;

        let logger = sdk::logger::Logger::new(sdk::logger::NoopSink);
        dispatch(
            &logger,
            KanbanCommand::PublicBoard {
                action: public_boards::PublicBoardAction::List {
                    project_id: "proj_1".into(),
                },
            },
            OutputFormat::Json,
            Some(&server.uri()),
        )
        .await
        .unwrap();
    }

    #[test]
    fn cold_start_transport_matches_unavailable_and_send_failures() {
        let connect = SunbeamError::from(connectrpc::ConnectError::new(
            connectrpc::ErrorCode::Unavailable,
            "error sending request for url (https://kanban.sunbeam.test/)",
        ));
        assert!(is_cold_start_transport(&connect));
        // sdk v3.3.0 routes ConnectRPC failures through the structured
        // `Connect` variant; a bare `Network` error is not retried.
        let plain =
            SunbeamError::network("error sending request for url (https://kanban.sunbeam.test/)");
        assert!(!is_cold_start_transport(&plain));
        let invalid = SunbeamError::from(connectrpc::ConnectError::new(
            connectrpc::ErrorCode::InvalidArgument,
            "invalid column_id",
        ));
        assert!(!is_cold_start_transport(&invalid));
        // A server-side `unavailable` application error is not a transport
        // failure and must not be retried.
        let server_side = SunbeamError::from(connectrpc::ConnectError::new(
            connectrpc::ErrorCode::Unavailable,
            "database unavailable",
        ));
        assert!(!is_cold_start_transport(&server_side));
        assert!(!is_cold_start_transport(&SunbeamError::Other(
            "no project matches".into()
        )));
    }

    #[tokio::test]
    async fn dispatch_retries_once_on_transport_unavailable() {
        use sdk::kanban::v1;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        // One-shot `unavailable` mounted first: wiremock prefers the earliest
        // mounted matching mock, so attempt one fails and the retry falls
        // through to the success responder below.
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/ListCardsByBoard"))
            .respond_with(testutil::connect_error(
                503,
                "unavailable",
                "error sending request for url (http://localhost/)",
            ))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/ListCardsByBoard"))
            .respond_with(testutil::proto_response(
                &v1::ListCardsByBoardResponse::default(),
            ))
            .mount(&server)
            .await;

        let logger = sdk::logger::Logger::new(sdk::logger::NoopSink);
        let uri = server.uri();
        retry_once_on_transport(&logger, || {
            let client = testutil::client_for(&uri);
            async move {
                dispatch_authed(
                    KanbanCommand::Card {
                        action: cards::CardAction::List {
                            board: "board_1".into(),
                            column: None,
                        },
                    },
                    OutputFormat::Table,
                    &client,
                )
                .await
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dispatch_does_not_retry_application_errors() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sunbeam.kanban.v1.CardService/ListCardsByBoard"))
            .respond_with(testutil::connect_error(
                400,
                "invalid_argument",
                "invalid board_id",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let logger = sdk::logger::Logger::new(sdk::logger::NoopSink);
        let uri = server.uri();
        let err = retry_once_on_transport(&logger, || {
            let client = testutil::client_for(&uri);
            async move {
                dispatch_authed(
                    KanbanCommand::Card {
                        action: cards::CardAction::List {
                            board: "board_1".into(),
                            column: None,
                        },
                    },
                    OutputFormat::Table,
                    &client,
                )
                .await
            }
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid board_id"));
    }
}
