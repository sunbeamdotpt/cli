//! Kanban subcommand — talk to the Sunbeam Kanban backend over gRPC.

use crate::error::{Result, ResultExt, SunbeamError};
use crate::logger::Logger;
use crate::output::OutputFormat;
use clap::Subcommand;

pub mod aggregated;
pub mod attachments;
pub mod auth;
pub mod boards;
pub mod card_templates;
pub mod cards;
pub mod client;
pub mod github_links;
pub mod projects;
pub mod public_boards;
pub mod search;
pub mod subscribe;
pub mod templates;

/// Generate a fresh ULID idempotency key for mutating RPCs.
pub fn new_idempotency_key() -> String {
    ulid::Ulid::new().to_string()
}

/// Resolve the default Kanban server URL from the provided context.
pub fn default_server_url_for(ctx: &crate::config::Context) -> Result<String> {
    let domain = ctx.domain.clone();
    if domain.is_empty() {
        return Err(SunbeamError::config(
            "no domain configured; set one with `sunbeam config set --domain ...` or pass --url",
        ));
    }
    Ok(format!("https://kanban.{domain}"))
}

/// Resolve the default Kanban server URL from the active context.
pub fn default_server_url() -> Result<String> {
    default_server_url_for(crate::config::active_context())
}

/// Resolve the final server URL from an explicit override or the active context.
pub fn resolve_server_url(url_override: Option<&str>) -> Result<String> {
    match url_override {
        Some(u) => Ok(u.to_string()),
        None => default_server_url(),
    }
}

/// Top-level `kanban` subcommands.
#[derive(Debug, Subcommand)]
#[command(name = "kanban")]
pub enum KanbanCommand {
    /// Authentication — whoami, logout.
    Auth {
        /// Auth subcommand to run.
        #[command(subcommand)]
        action: auth::AuthAction,
    },
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
    /// Board and card templates.
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
    Search(search::SearchAction),
    /// Public board read access (unauthenticated).
    #[command(name = "public-board")]
    PublicBoard {
        /// Public board subcommand to run.
        #[command(subcommand)]
        action: public_boards::PublicBoardAction,
    },
    /// Realtime event subscriptions.
    Subscribe {
        /// Subscribe subcommand to run.
        #[command(subcommand)]
        action: subscribe::SubscribeAction,
    },
}

/// Dispatch a kanban subcommand.
#[tracing::instrument(skip(logger))]
pub async fn dispatch(
    logger: &Logger,
    cmd: KanbanCommand,
    format: OutputFormat,
    url_override: Option<&str>,
) -> Result<()> {
    let server = resolve_server_url(url_override)?;
    crate::info!(
        logger,
        "kanban dispatch",
        server = server.to_string(),
        cmd = format!("{:?}", cmd)
    );

    match cmd {
        KanbanCommand::Auth { action } => {
            let token = require_token().await?;
            let mut client = auth::build_client(logger, &server, &token).await?;
            auth::run(action, format, &mut client).await
        }
        KanbanCommand::Project { action } => {
            let token = require_token().await?;
            let mut client = projects::build_client(logger, &server, &token).await?;
            projects::run(action, format, &mut client).await
        }
        KanbanCommand::Board { action } => {
            let token = require_token().await?;
            let mut client = boards::build_client(logger, &server, &token).await?;
            boards::run(action, format, &mut client).await
        }
        KanbanCommand::Aggregate { action } => {
            let token = require_token().await?;
            let mut client = aggregated::build_client(logger, &server, &token).await?;
            aggregated::run(action, format, &mut client).await
        }
        KanbanCommand::Card { action } => {
            let token = require_token().await?;
            let mut client = cards::build_client(logger, &server, &token).await?;
            cards::run(action, format, &mut client).await
        }
        KanbanCommand::Template { action } => {
            let token = require_token().await?;
            let mut client = templates::build_client(logger, &server, &token).await?;
            templates::run(action, format, &mut client).await
        }
        KanbanCommand::CardTemplate { action } => {
            let token = require_token().await?;
            let mut client = card_templates::build_client(logger, &server, &token).await?;
            card_templates::run(action, format, &mut client).await
        }
        KanbanCommand::Attachment { action } => {
            let token = require_token().await?;
            let mut client = attachments::build_client(logger, &server, &token).await?;
            attachments::run(action, format, &mut client).await
        }
        KanbanCommand::GitHub { action } => {
            let token = require_token().await?;
            let mut client = github_links::build_client(logger, &server, &token).await?;
            github_links::run(action, format, &mut client).await
        }
        KanbanCommand::Search(action) => {
            let token = require_token().await?;
            let mut client = search::build_client(logger, &server, &token).await?;
            search::run(action, format, &mut client).await
        }
        KanbanCommand::PublicBoard { action } => {
            let mut client = public_boards::build_client(logger, &server).await?;
            public_boards::run_with_client(action, format, &mut client).await
        }
        KanbanCommand::Subscribe { action } => {
            let token = require_token().await?;
            let mut client = subscribe::build_client(logger, &server, &token).await?;
            subscribe::run_with_client(action, &mut client).await
        }
    }
}

/// Resolve and validate a bearer token for authenticated RPCs.
pub async fn require_token() -> Result<String> {
    crate::auth::get_token()
        .await
        .with_ctx(|| "run `sunbeam auth login` first".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> crate::cli::Cli {
        crate::cli::Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn kanban_project_list_parses() {
        let cli = parse(&["sunbeam", "kanban", "project", "list"]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban {
                action:
                    KanbanCommand::Project {
                        action: projects::ProjectAction::List,
                    },
                ..
            }) => {}
            other => panic!("expected kanban project list, got {other:?}"),
        }
    }

    #[test]
    fn kanban_board_list_with_project_parses() {
        let cli = parse(&[
            "sunbeam",
            "kanban",
            "board",
            "list",
            "--project",
            "proj_123",
        ]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban {
                action:
                    KanbanCommand::Board {
                        action: boards::BoardAction::List { project },
                    },
                ..
            }) => {
                assert_eq!(project, "proj_123");
            }
            other => panic!("expected kanban board list, got {other:?}"),
        }
    }

    #[test]
    fn kanban_card_create_parses() {
        let cli = parse(&[
            "sunbeam",
            "kanban",
            "card",
            "create",
            "--board",
            "board_123",
            "--title",
            "Fix it",
            "--priority",
            "high",
        ]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban {
                action:
                    KanbanCommand::Card {
                        action:
                            cards::CardAction::Create {
                                board,
                                title,
                                priority: Some(cards::PriorityArg::High),
                                ..
                            },
                    },
                ..
            }) => {
                assert_eq!(board, "board_123");
                assert_eq!(title, "Fix it");
            }
            other => panic!("expected kanban card create, got {other:?}"),
        }
    }

    #[test]
    fn kanban_search_parses() {
        let cli = parse(&["sunbeam", "kanban", "search", "frontend crash"]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban {
                action: KanbanCommand::Search(search::SearchAction { query, limit }),
                ..
            }) => {
                assert_eq!(query, "frontend crash");
                assert!(limit.is_none());
            }
            other => panic!("expected kanban search, got {other:?}"),
        }
    }

    #[test]
    fn kanban_public_board_get_parses() {
        let cli = parse(&["sunbeam", "kanban", "public-board", "get", "board_123"]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban {
                action:
                    KanbanCommand::PublicBoard {
                        action: public_boards::PublicBoardAction::Get { board_id },
                    },
                ..
            }) => assert_eq!(board_id, "board_123"),
            other => panic!("expected kanban public-board get, got {other:?}"),
        }
    }

    #[test]
    fn kanban_subscribe_board_parses() {
        let cli = parse(&["sunbeam", "kanban", "subscribe", "board", "board_123"]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban {
                action:
                    KanbanCommand::Subscribe {
                        action: subscribe::SubscribeAction::Board { board_id },
                    },
                ..
            }) => assert_eq!(board_id, "board_123"),
            other => panic!("expected kanban subscribe board, got {other:?}"),
        }
    }

    #[test]
    fn kanban_url_override_parses() {
        let cli = parse(&[
            "sunbeam",
            "kanban",
            "--url",
            "http://localhost:8080",
            "project",
            "list",
        ]);
        match cli.verb {
            Some(crate::cli::Verb::Kanban { url: Some(u), .. }) => {
                assert_eq!(u, "http://localhost:8080");
            }
            other => panic!("expected kanban with --url, got {other:?}"),
        }
    }

    #[test]
    fn resolve_server_url_uses_override() {
        assert_eq!(
            resolve_server_url(Some("http://local")).unwrap(),
            "http://local"
        );
    }

    #[test]
    fn default_server_url_for_builds_from_domain() {
        let ctx = crate::config::Context {
            domain: "sunbeam.test".into(),
            ..Default::default()
        };
        assert_eq!(
            default_server_url_for(&ctx).unwrap(),
            "https://kanban.sunbeam.test"
        );
    }

    #[test]
    fn default_server_url_for_errors_when_domain_empty() {
        let ctx = crate::config::Context::default();
        assert!(default_server_url_for(&ctx).is_err());
    }

    #[test]
    fn new_idempotency_key_is_ulid() {
        let key = new_idempotency_key();
        assert!(!key.is_empty());
        assert!(key.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn default_server_url_errors_when_domain_empty() {
        crate::config::set_active_context(crate::config::Context::default());
        let err = default_server_url().unwrap_err();
        assert!(err.to_string().contains("no domain configured"));
    }

    #[tokio::test]
    async fn dispatch_public_board_rejects_invalid_url() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = dispatch(
            &logger,
            KanbanCommand::PublicBoard {
                action: public_boards::PublicBoardAction::Get {
                    board_id: "b".into(),
                },
            },
            OutputFormat::Json,
            Some(":::not-a-url"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid kanban server URL"));
    }

    #[tokio::test]
    async fn dispatch_auth_rejects_missing_token() {
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        let err = dispatch(
            &logger,
            KanbanCommand::Auth {
                action: auth::AuthAction::WhoAmI,
            },
            OutputFormat::Json,
            Some("http://localhost:8080"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("run `sunbeam auth login` first"));
    }
}
