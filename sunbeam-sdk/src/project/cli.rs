//! Dispatch for `sunbeam project` subcommands.

use std::collections::BTreeMap;
use std::path::PathBuf;

use tokio::task::JoinSet;

use crate::cli::{ProjectAction, ProjectRunArgs};
use crate::discovery::{find_project_root, find_workspace_root, WORKSPACE_FILE};
use crate::error::{Result, SunbeamError};
use crate::operations::config::WorkspaceConfig;
use crate::output::{ok, warn};
use crate::project::config::ProjectConfig;
use crate::project::runner::{RunOptions, RunOutcome};
use crate::topo::{sort, Graph};

pub async fn dispatch(action: ProjectAction) -> Result<()> {
    match action {
        ProjectAction::Build(args) => run_verb("build", args).await,
        ProjectAction::Test(args) => run_verb("test", args).await,
        ProjectAction::Lint(args) => run_verb("lint", args).await,
        ProjectAction::Fmt(args) => run_verb("fmt", args).await,
        ProjectAction::Package(args) => run_verb("package", args).await,
        ProjectAction::Deploy(args) => run_verb("deploy", args).await,
        ProjectAction::Dev(args) => run_verb("dev", args).await,
        ProjectAction::Clean(args) => run_verb("clean", args).await,
        ProjectAction::Doc(args) => run_verb("doc", args).await,
        ProjectAction::Info => cmd_info().await,
        ProjectAction::Order { verb } => cmd_order(&verb).await,
    }
}

async fn run_verb(verb: &str, args: ProjectRunArgs) -> Result<()> {
    let opts = RunOptions {
        extra_env: BTreeMap::new(),
        verbose: args.verbose,
        dry_run: args.dry_run,
    };

    if args.all || !args.projects.is_empty() {
        run_workspace(verb, &args, opts).await
    } else {
        run_single(verb, opts).await
    }
}

async fn run_single(verb: &str, opts: RunOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = find_project_root(&cwd)?;
    let cfg = ProjectConfig::load(&project_root.join("sunbeam.yaml"))?;
    let outcome = crate::project::runner::run(&cfg, &project_root, verb, &opts).await?;
    if matches!(outcome, RunOutcome::Skipped) {
        warn(&format!("  skipped (no {verb} target)"));
    }
    Ok(())
}

async fn run_workspace(verb: &str, args: &ProjectRunArgs, opts: RunOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let ws_root = find_workspace_root(&cwd)?;
    let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))?;

    // Collect project roots and configs.
    let all_entries: Vec<(String, PathBuf, ProjectConfig)> = ws
        .iter_repos()
        .filter_map(|entry| {
            // Only owned repos are expected to have sunbeam.yaml manifests.
            use crate::operations::config::RepoBucket;
            if entry.bucket != RepoBucket::Owned {
                return None;
            }
            let abs = ws_root.join(&entry.repo.path);
            let manifest = abs.join("sunbeam.yaml");
            if !manifest.exists() {
                return None;
            }
            ProjectConfig::load(&manifest)
                .ok()
                .map(|cfg| (entry.name.to_string(), abs, cfg))
        })
        .collect();

    // Filter by --project flags if given.
    let entries: Vec<(String, PathBuf, ProjectConfig)> = if args.projects.is_empty() {
        all_entries
    } else {
        let wanted: std::collections::BTreeSet<&str> =
            args.projects.iter().map(|s| s.as_str()).collect();
        all_entries
            .into_iter()
            .filter(|(name, _, _)| wanted.contains(name.as_str()))
            .collect()
    };

    // Build dep graph and sort.
    let mut graph: Graph = BTreeMap::new();
    let name_to_entry: BTreeMap<String, (PathBuf, ProjectConfig)> = entries
        .into_iter()
        .map(|(name, path, cfg)| {
            graph.insert(name.clone(), cfg.deps.projects.clone());
            (name, (path, cfg))
        })
        .collect();

    let sorted = sort(&graph).map_err(|e| SunbeamError::Other(e.to_string()))?;

    for group in &sorted.0 {
        let mut set: JoinSet<Result<(String, RunOutcome)>> = JoinSet::new();

        for project_name in group {
            let Some((project_root, cfg)) = name_to_entry.get(project_name) else {
                continue;
            };
            let project_root = project_root.clone();
            let cfg = cfg.clone();
            let verb_s = verb.to_string();
            let task_opts = RunOptions {
                extra_env: opts.extra_env.clone(),
                verbose: opts.verbose,
                dry_run: opts.dry_run,
            };
            let project_name = project_name.clone();
            set.spawn(async move {
                let outcome =
                    crate::project::runner::run(&cfg, &project_root, &verb_s, &task_opts).await?;
                Ok((project_name, outcome))
            });
        }

        while let Some(res) = set.join_next().await {
            let (name, outcome) = res
                .map_err(|e| SunbeamError::Other(format!("task join error: {e}")))??;
            if matches!(outcome, RunOutcome::Skipped) {
                warn(&format!("  {name}: skipped (no {verb} target)"));
            }
        }
    }

    Ok(())
}

async fn cmd_info() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project_root = find_project_root(&cwd)?;
    let cfg = ProjectConfig::load(&project_root.join("sunbeam.yaml"))?;
    print!("{}", serde_yaml::to_string(&cfg)?);
    Ok(())
}

async fn cmd_order(verb: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let ws_root = find_workspace_root(&cwd)?;
    let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))?;

    let mut graph: Graph = BTreeMap::new();
    let mut has_target: BTreeMap<String, bool> = BTreeMap::new();

    for entry in ws.iter_repos() {
        use crate::operations::config::RepoBucket;
        if entry.bucket != RepoBucket::Owned {
            continue;
        }
        let abs = ws_root.join(&entry.repo.path);
        let manifest = abs.join("sunbeam.yaml");
        if !manifest.exists() {
            continue;
        }
        if let Ok(cfg) = ProjectConfig::load(&manifest) {
            let name = entry.name.to_string();
            let active = cfg.has_target(verb);
            graph.insert(name.clone(), cfg.deps.projects.clone());
            has_target.insert(name, active);
        }
    }

    let sorted = sort(&graph).map_err(|e| SunbeamError::Other(e.to_string()))?;

    for (i, group) in sorted.0.iter().enumerate() {
        let tokens: Vec<String> = group
            .iter()
            .map(|name| {
                if *has_target.get(name).unwrap_or(&false) {
                    name.clone()
                } else {
                    format!("{name} (skip)")
                }
            })
            .collect();
        ok(&format!("group {i}: {}", tokens.join("  ")));
    }

    Ok(())
}
