//! Kanban subcommand dispatch — talk to the Sunbeam Kanban backend over gRPC.

use clap::Subcommand;
use sunbeam_sdk::error::{Result, ResultExt};
use sunbeam_sdk::logger::Logger;
use sunbeam_sdk::output::OutputFormat;

/// Top-level `kanban` subcommands.
#[derive(Debug, Subcommand)]
#[command(name = "kanban")]
pub enum KanbanCommand {
    /// Project management.
    Project {
        /// Project subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::projects::ProjectAction,
    },
    /// Board management.
    Board {
        /// Board subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::boards::BoardAction,
    },
    /// Aggregated (meta) board management.
    Aggregate {
        /// Aggregated board subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::aggregated::AggregateAction,
    },
    /// Card management.
    Card {
        /// Card subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::cards::CardAction,
    },
    /// Board and card templates.
    Template {
        /// Template subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::templates::TemplateAction,
    },
    /// Card templates.
    #[command(name = "card-template")]
    CardTemplate {
        /// Card template subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::card_templates::CardTemplateAction,
    },
    /// Attachment management.
    Attachment {
        /// Attachment subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::attachments::AttachmentAction,
    },
    /// GitHub issue/PR links.
    #[command(name = "github")]
    GitHub {
        /// GitHub link subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::github_links::GitHubAction,
    },
    /// Full-text card search.
    Search(sunbeam_sdk::kanban::search::SearchAction),
    /// Public board read access (unauthenticated).
    #[command(name = "public-board")]
    PublicBoard {
        /// Public board subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::public_boards::PublicBoardAction,
    },
    /// Realtime event subscriptions.
    Subscribe {
        /// Subscribe subcommand to run.
        #[command(subcommand)]
        action: sunbeam_sdk::kanban::subscribe::SubscribeAction,
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
    let server = sunbeam_sdk::kanban::resolve_server_url(url_override)?;
    sunbeam_sdk::info!(
        logger,
        "kanban dispatch",
        server = server.to_string(),
        cmd = format!("{:?}", cmd)
    );

    match cmd {
        KanbanCommand::Project { action } => {
            let token = require_token().await?;
            let mut client =
                sunbeam_sdk::kanban::projects::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::projects::run(action, format, &mut client).await
        }
        KanbanCommand::Board { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::boards::BoardAction::List { project } => {
                    sunbeam_sdk::kanban::boards::BoardAction::List {
                        project: resolver.project(&project).await?,
                    }
                }
                sunbeam_sdk::kanban::boards::BoardAction::Get { board_id } => {
                    sunbeam_sdk::kanban::boards::BoardAction::Get {
                        board_id: resolver.board_anywhere(&board_id).await?,
                    }
                }
                sunbeam_sdk::kanban::boards::BoardAction::Create {
                    project,
                    name,
                    description,
                    icon,
                    visibility,
                } => sunbeam_sdk::kanban::boards::BoardAction::Create {
                    project: resolver.project(&project).await?,
                    name,
                    description,
                    icon,
                    visibility,
                },
                sunbeam_sdk::kanban::boards::BoardAction::Update {
                    board_id,
                    name,
                    description,
                    icon,
                    visibility,
                } => sunbeam_sdk::kanban::boards::BoardAction::Update {
                    board_id: resolver.board_anywhere(&board_id).await?,
                    name,
                    description,
                    icon,
                    visibility,
                },
                sunbeam_sdk::kanban::boards::BoardAction::Delete { board_id } => {
                    sunbeam_sdk::kanban::boards::BoardAction::Delete {
                        board_id: resolver.board_anywhere(&board_id).await?,
                    }
                }
                sunbeam_sdk::kanban::boards::BoardAction::Column { action } => {
                    let action = match action {
                        sunbeam_sdk::kanban::boards::ColumnAction::Add {
                            board_id,
                            title,
                            accent,
                            wip_limit,
                            position,
                        } => sunbeam_sdk::kanban::boards::ColumnAction::Add {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            title,
                            accent,
                            wip_limit,
                            position,
                        },
                        sunbeam_sdk::kanban::boards::ColumnAction::Update {
                            board_id,
                            column_id,
                            title,
                            accent,
                            wip_limit,
                        } => sunbeam_sdk::kanban::boards::ColumnAction::Update {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            column_id,
                            title,
                            accent,
                            wip_limit,
                        },
                        sunbeam_sdk::kanban::boards::ColumnAction::Remove {
                            board_id,
                            column_id,
                        } => sunbeam_sdk::kanban::boards::ColumnAction::Remove {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            column_id,
                        },
                        sunbeam_sdk::kanban::boards::ColumnAction::Move {
                            board_id,
                            column_id,
                            position,
                        } => sunbeam_sdk::kanban::boards::ColumnAction::Move {
                            board_id: resolver.board_anywhere(&board_id).await?,
                            column_id,
                            position,
                        },
                    };
                    sunbeam_sdk::kanban::boards::BoardAction::Column { action }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::boards::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::boards::run(action, format, &mut client).await
        }
        KanbanCommand::Aggregate { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::aggregated::AggregateAction::List => {
                    sunbeam_sdk::kanban::aggregated::AggregateAction::List
                }
                sunbeam_sdk::kanban::aggregated::AggregateAction::Get { aggregate_id } => {
                    sunbeam_sdk::kanban::aggregated::AggregateAction::Get { aggregate_id }
                }
                sunbeam_sdk::kanban::aggregated::AggregateAction::Create {
                    name,
                    description,
                    icon,
                    visibility,
                } => sunbeam_sdk::kanban::aggregated::AggregateAction::Create {
                    name,
                    description,
                    icon,
                    visibility,
                },
                sunbeam_sdk::kanban::aggregated::AggregateAction::Update {
                    aggregate_id,
                    name,
                    description,
                    icon,
                } => sunbeam_sdk::kanban::aggregated::AggregateAction::Update {
                    aggregate_id,
                    name,
                    description,
                    icon,
                },
                sunbeam_sdk::kanban::aggregated::AggregateAction::Delete { aggregate_id } => {
                    sunbeam_sdk::kanban::aggregated::AggregateAction::Delete { aggregate_id }
                }
                sunbeam_sdk::kanban::aggregated::AggregateAction::Source { action } => {
                    let action = match action {
                        sunbeam_sdk::kanban::aggregated::SourceAction::Add {
                            aggregate_id,
                            board_id,
                            position,
                        } => sunbeam_sdk::kanban::aggregated::SourceAction::Add {
                            aggregate_id,
                            board_id: resolver.board_anywhere(&board_id).await?,
                            position,
                        },
                        sunbeam_sdk::kanban::aggregated::SourceAction::Remove {
                            aggregate_id,
                            board_id,
                        } => sunbeam_sdk::kanban::aggregated::SourceAction::Remove {
                            aggregate_id,
                            board_id: resolver.board_anywhere(&board_id).await?,
                        },
                        sunbeam_sdk::kanban::aggregated::SourceAction::Move {
                            aggregate_id,
                            board_id,
                            position,
                        } => sunbeam_sdk::kanban::aggregated::SourceAction::Move {
                            aggregate_id,
                            board_id: resolver.board_anywhere(&board_id).await?,
                            position,
                        },
                    };
                    sunbeam_sdk::kanban::aggregated::AggregateAction::Source { action }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::aggregated::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::aggregated::run(action, format, &mut client).await
        }
        KanbanCommand::Card { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::cards::CardAction::List { board, column } => {
                    sunbeam_sdk::kanban::cards::CardAction::List {
                        board: resolver.board_anywhere(&board).await?,
                        column,
                    }
                }
                sunbeam_sdk::kanban::cards::CardAction::Get { card_id } => {
                    sunbeam_sdk::kanban::cards::CardAction::Get {
                        card_id: resolver.card_anywhere(&card_id).await?,
                    }
                }
                sunbeam_sdk::kanban::cards::CardAction::Create {
                    board,
                    column,
                    title,
                    description,
                    priority,
                } => sunbeam_sdk::kanban::cards::CardAction::Create {
                    board: resolver.board_anywhere(&board).await?,
                    column,
                    title,
                    description,
                    priority,
                },
                sunbeam_sdk::kanban::cards::CardAction::Update {
                    card_id,
                    title,
                    description,
                    priority,
                } => sunbeam_sdk::kanban::cards::CardAction::Update {
                    card_id: resolver.card_anywhere(&card_id).await?,
                    title,
                    description,
                    priority,
                },
                sunbeam_sdk::kanban::cards::CardAction::Move {
                    card_id,
                    column,
                    position,
                } => sunbeam_sdk::kanban::cards::CardAction::Move {
                    card_id: resolver.card_anywhere(&card_id).await?,
                    column,
                    position,
                },
                sunbeam_sdk::kanban::cards::CardAction::Delete { card_id } => {
                    sunbeam_sdk::kanban::cards::CardAction::Delete {
                        card_id: resolver.card_anywhere(&card_id).await?,
                    }
                }
                sunbeam_sdk::kanban::cards::CardAction::Dependency { action } => {
                    let action = match action {
                        sunbeam_sdk::kanban::cards::DependencyAction::Add {
                            board,
                            card_id,
                            depends_on,
                        } => sunbeam_sdk::kanban::cards::DependencyAction::Add {
                            board: resolver.board_anywhere(&board).await?,
                            card_id: resolver.card_anywhere(&card_id).await?,
                            depends_on: resolver.card_anywhere(&depends_on).await?,
                        },
                        sunbeam_sdk::kanban::cards::DependencyAction::Remove {
                            board,
                            card_id,
                            depends_on,
                        } => sunbeam_sdk::kanban::cards::DependencyAction::Remove {
                            board: resolver.board_anywhere(&board).await?,
                            card_id: resolver.card_anywhere(&card_id).await?,
                            depends_on: resolver.card_anywhere(&depends_on).await?,
                        },
                    };
                    sunbeam_sdk::kanban::cards::CardAction::Dependency { action }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::cards::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::cards::run(action, format, &mut client).await
        }
        KanbanCommand::Template { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::templates::TemplateAction::List { project } => {
                    sunbeam_sdk::kanban::templates::TemplateAction::List {
                        project: match project {
                            Some(p) => Some(resolver.project(&p).await?),
                            None => None,
                        },
                    }
                }
                sunbeam_sdk::kanban::templates::TemplateAction::Get { template_id } => {
                    sunbeam_sdk::kanban::templates::TemplateAction::Get { template_id }
                }
                sunbeam_sdk::kanban::templates::TemplateAction::Create {
                    project,
                    name,
                    description,
                } => sunbeam_sdk::kanban::templates::TemplateAction::Create {
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    name,
                    description,
                },
                sunbeam_sdk::kanban::templates::TemplateAction::Update {
                    template_id,
                    name,
                    description,
                } => sunbeam_sdk::kanban::templates::TemplateAction::Update {
                    template_id,
                    name,
                    description,
                },
                sunbeam_sdk::kanban::templates::TemplateAction::Delete { template_id } => {
                    sunbeam_sdk::kanban::templates::TemplateAction::Delete { template_id }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::templates::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::templates::run(action, format, &mut client).await
        }
        KanbanCommand::CardTemplate { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::card_templates::CardTemplateAction::List { project } => {
                    sunbeam_sdk::kanban::card_templates::CardTemplateAction::List {
                        project: match project {
                            Some(p) => Some(resolver.project(&p).await?),
                            None => None,
                        },
                    }
                }
                sunbeam_sdk::kanban::card_templates::CardTemplateAction::Get { template_id } => {
                    sunbeam_sdk::kanban::card_templates::CardTemplateAction::Get { template_id }
                }
                sunbeam_sdk::kanban::card_templates::CardTemplateAction::Create {
                    project,
                    name,
                } => sunbeam_sdk::kanban::card_templates::CardTemplateAction::Create {
                    project: match project {
                        Some(p) => Some(resolver.project(&p).await?),
                        None => None,
                    },
                    name,
                },
                sunbeam_sdk::kanban::card_templates::CardTemplateAction::Update {
                    template_id,
                    name,
                } => sunbeam_sdk::kanban::card_templates::CardTemplateAction::Update {
                    template_id,
                    name,
                },
                sunbeam_sdk::kanban::card_templates::CardTemplateAction::Delete { template_id } => {
                    sunbeam_sdk::kanban::card_templates::CardTemplateAction::Delete { template_id }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::card_templates::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::card_templates::run(action, format, &mut client).await
        }
        KanbanCommand::Attachment { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::attachments::AttachmentAction::List { card_id } => {
                    sunbeam_sdk::kanban::attachments::AttachmentAction::List {
                        card_id: resolver.card_anywhere(&card_id).await?,
                    }
                }
                sunbeam_sdk::kanban::attachments::AttachmentAction::Upload { card_id, file } => {
                    sunbeam_sdk::kanban::attachments::AttachmentAction::Upload {
                        card_id: resolver.card_anywhere(&card_id).await?,
                        file,
                    }
                }
                sunbeam_sdk::kanban::attachments::AttachmentAction::Download {
                    card,
                    attachment_id,
                    path,
                } => sunbeam_sdk::kanban::attachments::AttachmentAction::Download {
                    card: resolver.card_anywhere(&card).await?,
                    attachment_id,
                    path,
                },
                sunbeam_sdk::kanban::attachments::AttachmentAction::Delete {
                    card,
                    attachment_id,
                } => sunbeam_sdk::kanban::attachments::AttachmentAction::Delete {
                    card: resolver.card_anywhere(&card).await?,
                    attachment_id,
                },
            };
            let mut client =
                sunbeam_sdk::kanban::attachments::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::attachments::run(action, format, &mut client).await
        }
        KanbanCommand::GitHub { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::github_links::GitHubAction::Link { card_id, issue } => {
                    sunbeam_sdk::kanban::github_links::GitHubAction::Link {
                        card_id: resolver.card_anywhere(&card_id).await?,
                        issue,
                    }
                }
                sunbeam_sdk::kanban::github_links::GitHubAction::Unlink { card, link_id } => {
                    sunbeam_sdk::kanban::github_links::GitHubAction::Unlink {
                        card: resolver.card_anywhere(&card).await?,
                        link_id,
                    }
                }
                sunbeam_sdk::kanban::github_links::GitHubAction::List { card_id } => {
                    sunbeam_sdk::kanban::github_links::GitHubAction::List {
                        card_id: resolver.card_anywhere(&card_id).await?,
                    }
                }
                sunbeam_sdk::kanban::github_links::GitHubAction::Search { card, repo, query } => {
                    sunbeam_sdk::kanban::github_links::GitHubAction::Search {
                        card: resolver.card_anywhere(&card).await?,
                        repo,
                        query,
                    }
                }
                sunbeam_sdk::kanban::github_links::GitHubAction::Resync { card, link_id } => {
                    sunbeam_sdk::kanban::github_links::GitHubAction::Resync {
                        card: resolver.card_anywhere(&card).await?,
                        link_id,
                    }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::github_links::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::github_links::run(action, format, &mut client).await
        }
        KanbanCommand::Search(action) => {
            let token = require_token().await?;
            let mut client =
                sunbeam_sdk::kanban::search::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::search::run(action, format, &mut client).await
        }
        KanbanCommand::PublicBoard { action } => {
            let action = match action {
                sunbeam_sdk::kanban::public_boards::PublicBoardAction::Get { board_id } => {
                    if sunbeam_sdk::kanban::resolve::looks_like_id(&board_id) {
                        sunbeam_sdk::kanban::public_boards::PublicBoardAction::Get { board_id }
                    } else {
                        let token = require_token().await?;
                        let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(
                            logger, &server, &token,
                        );
                        sunbeam_sdk::kanban::public_boards::PublicBoardAction::Get {
                            board_id: resolver.public_board_anywhere(&board_id).await?,
                        }
                    }
                }
                sunbeam_sdk::kanban::public_boards::PublicBoardAction::List { project_id } => {
                    if sunbeam_sdk::kanban::resolve::looks_like_id(&project_id) {
                        sunbeam_sdk::kanban::public_boards::PublicBoardAction::List { project_id }
                    } else {
                        let token = require_token().await?;
                        let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(
                            logger, &server, &token,
                        );
                        sunbeam_sdk::kanban::public_boards::PublicBoardAction::List {
                            project_id: resolver.project(&project_id).await?,
                        }
                    }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::public_boards::build_client(logger, &server).await?;
            sunbeam_sdk::kanban::public_boards::run_with_client(action, format, &mut client).await
        }
        KanbanCommand::Subscribe { action } => {
            let token = require_token().await?;
            let resolver = sunbeam_sdk::kanban::resolve::NameResolver::new(logger, &server, &token);
            let action = match action {
                sunbeam_sdk::kanban::subscribe::SubscribeAction::Board { board_id } => {
                    sunbeam_sdk::kanban::subscribe::SubscribeAction::Board {
                        board_id: resolver.board_anywhere(&board_id).await?,
                    }
                }
                sunbeam_sdk::kanban::subscribe::SubscribeAction::Project { project_id } => {
                    sunbeam_sdk::kanban::subscribe::SubscribeAction::Project {
                        project_id: resolver.project(&project_id).await?,
                    }
                }
            };
            let mut client =
                sunbeam_sdk::kanban::subscribe::build_client(logger, &server, &token).await?;
            sunbeam_sdk::kanban::subscribe::run_with_client(action, &mut client).await
        }
    }
}

/// Resolve and validate a bearer token for authenticated RPCs.
async fn require_token() -> Result<String> {
    sunbeam_sdk::auth::get_token()
        .await
        .with_ctx(|| "run `sunbeam auth login` first".to_string())
}
