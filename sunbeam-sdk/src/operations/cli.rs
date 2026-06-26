//! Dispatch for `sunbeam operations` subcommands.

use crate::cli::{ComposeAction, OperationsAction, StackAction};
use crate::discovery::{WORKSPACE_FILE, find_workspace_root};
use crate::error::Result;
use crate::operations::compose::ComposeOptions;
use crate::operations::config::{RepoBucket, WorkspaceConfig};
use crate::{debug, info};

/// Dispatch.
#[tracing::instrument(skip(logger))]
pub async fn dispatch(logger: &crate::logger::Logger, action: OperationsAction) -> Result<()> {
    debug!(
        logger,
        "operations dispatch",
        action = format!("{:?}", action)
    );
    info!(
        logger,
        "operations dispatch",
        action = format!("{:?}", action)
    );
    match action {
        OperationsAction::Compose { action } => {
            let (ws, ws_root) = load_workspace().await?;
            dispatch_compose(logger, action, ws, ws_root).await
        }
        OperationsAction::Stack { action } => {
            let (ws, ws_root) = load_workspace().await?;
            dispatch_stack(logger, action, ws, ws_root).await
        }
        OperationsAction::Info => {
            let (ws, _) = load_workspace().await?;
            print!("{}", serde_yaml::to_string(&ws)?);
            Ok(())
        }
        OperationsAction::Repos => {
            let (ws, _) = load_workspace().await?;
            let buckets = [
                RepoBucket::Owned,
                RepoBucket::ThirdParty,
                RepoBucket::Forks,
                RepoBucket::Research,
                RepoBucket::Retired,
            ];
            for bucket in buckets {
                let entries: Vec<_> = ws.iter_repos().filter(|e| e.bucket == bucket).collect();
                if entries.is_empty() {
                    continue;
                }
                info!(logger, "bucket", name = bucket.as_str());
                for e in entries {
                    info!(
                        logger,
                        "repo entry",
                        name = e.name,
                        path = e.repo.path.as_str()
                    );
                }
            }
            Ok(())
        }
    }
}

async fn load_workspace() -> Result<(WorkspaceConfig, std::path::PathBuf)> {
    let cwd = std::env::current_dir()?;
    let ws_root = find_workspace_root(&cwd)?;
    let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))?;
    Ok((ws, ws_root))
}

async fn dispatch_compose(
    logger: &crate::logger::Logger,
    action: ComposeAction,
    ws: WorkspaceConfig,
    ws_root: std::path::PathBuf,
) -> Result<()> {
    match action {
        ComposeAction::Render => {
            let path = crate::operations::compose::materialize(&ws, &ws_root)?;
            info!(logger, "wrote", path = path.display());
            Ok(())
        }
        ComposeAction::Up { services, wait } => {
            let opts = ComposeOptions {
                detach: true,
                wait,
                ..Default::default()
            };
            crate::operations::compose::up(&ws, &ws_root, &services, &opts).await
        }
        ComposeAction::Down { volumes } => {
            let opts = ComposeOptions {
                volumes,
                ..Default::default()
            };
            crate::operations::compose::down(&ws, &ws_root, &opts).await
        }
        ComposeAction::Ps => {
            let opts = ComposeOptions::default();
            let statuses = crate::operations::compose::ps(&ws, &ws_root, &opts).await?;
            print_ps_table(statuses);
            Ok(())
        }
        ComposeAction::Logs { service, follow } => {
            let opts = ComposeOptions::default();
            crate::operations::compose::logs(&ws, &ws_root, &service, follow, &opts).await
        }
    }
}

fn print_ps_table(statuses: Vec<crate::operations::compose::ServiceStatus>) {
    use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL};
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("SERVICE").fg(comfy_table::Color::Cyan),
            Cell::new("STATUS").fg(comfy_table::Color::Cyan),
            Cell::new("PORTS").fg(comfy_table::Color::Cyan),
        ]);
    for s in statuses {
        let health = s.health.unwrap_or_default();
        let state_col = if health.is_empty() {
            s.state
        } else {
            format!("{} ({health})", s.state)
        };
        table.add_row(vec![s.name, state_col, s.ports.unwrap_or_default()]);
    }
    println!("{table}");
}

async fn dispatch_stack(
    logger: &crate::logger::Logger,
    action: StackAction,
    mut ws: WorkspaceConfig,
    ws_root: std::path::PathBuf,
) -> Result<()> {
    match action {
        StackAction::List => {
            let summaries = crate::operations::stack::list(&ws);
            print_stack_table(summaries);
            Ok(())
        }
        StackAction::Pin {
            name,
            projects,
            description,
        } => {
            crate::operations::stack::pin(
                &mut ws,
                &ws_root,
                &name,
                description.as_deref(),
                &projects,
            )?;
            crate::operations::stack::save(&ws, &ws_root)
        }
        StackAction::Apply { name } => crate::operations::stack::apply(&ws, &ws_root, &name).await,
        StackAction::Diff { left, right } => {
            let entries = crate::operations::stack::diff(&ws, &ws_root, &left, right.as_deref())?;
            print_diff(logger, entries);
            Ok(())
        }
    }
}

fn print_stack_table(summaries: Vec<crate::operations::stack::StackSummary>) {
    use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL};
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("NAME").fg(comfy_table::Color::Cyan),
            Cell::new("PINNED AT").fg(comfy_table::Color::Cyan),
            Cell::new("DESCRIPTION").fg(comfy_table::Color::Cyan),
        ]);
    for s in summaries {
        table.add_row(vec![
            s.name,
            s.pinned_at.unwrap_or_default(),
            s.description.unwrap_or_default(),
        ]);
    }
    println!("{table}");
}

fn print_diff(
    logger: &crate::logger::Logger,
    entries: Vec<crate::operations::stack::StackDiffEntry>,
) {
    for e in entries {
        let left = e.left.as_deref().unwrap_or("(missing)");
        let right = e.right.as_deref().unwrap_or("(missing)");
        info!(
            logger,
            "diff entry",
            project = e.project,
            left = left,
            right = right
        );
    }
}
