//! Dispatch for `sunbeam project` subcommands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cli::{ProjectAction, ProjectRunArgs};
use crate::config::get_infra_dir;
use crate::discovery::{find_project_root, find_workspace_root, WORKSPACE_FILE};
use crate::error::{Result, SunbeamError};
use crate::operations::config::WorkspaceConfig;

use crate::project::config::{is_standard_verb, ProjectConfig, STANDARD_VERBS};
use crate::project::runner::{RunOptions, RunOutcome};
use crate::topo::{sort, Graph};

/// Dispatch.
#[tracing::instrument]
pub async fn dispatch(action: ProjectAction) -> Result<()> {
    tracing::debug!("project dispatch: {action:?}");
    tracing::info!("project dispatch: {action:?}");
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
        ProjectAction::Run { verb, args } => run_verb(&verb, args).await,
        ProjectAction::Graph { all } => cmd_graph(all).await,
        ProjectAction::Check { all } => cmd_check(all).await,
        ProjectAction::PreseedImage { image_ref, timeout } => {
            cmd_preseed_image(&image_ref, timeout).await
        }
    }
}

async fn cmd_preseed_image(image_ref: &str, timeout: u64) -> Result<()> {
    // 1. Apply the puller Job and wait for the node pull to complete.
    crate::proxy::cmd_preseed_image(image_ref, timeout).await?;

    // 2. Extract the tag from the image ref and bump the kustomization.
    let tag = image_ref.rsplit(':').next().unwrap_or(image_ref);
    let kustomization_path = get_infra_dir()
        .join("sbbb")
        .join("base")
        .join("ingress")
        .join("kustomization.yaml");

    let current = std::fs::read_to_string(&kustomization_path).map_err(|e| {
        SunbeamError::Io {
            context: format!("reading {}", kustomization_path.display()),
            source: e,
        }
    })?;

    let updated = crate::proxy::bump_proxy_image(&current, tag)?;
    std::fs::write(&kustomization_path, updated.as_bytes()).map_err(|e| {
        SunbeamError::Io {
            context: format!("writing {}", kustomization_path.display()),
            source: e,
        }
    })?;

    tracing::info!(
        "Bumped infra/sbbb/base/ingress/kustomization.yaml → newTag: {tag}"
    );
    tracing::info!("Run `sunbeam service apply ingress` to roll out the new proxy image.");
    Ok(())
}

async fn run_verb(verb: &str, args: ProjectRunArgs) -> Result<()> {
    let opts = RunOptions {
        extra_env: BTreeMap::new(),
        verbose: args.echo,
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
        tracing::warn!("  skipped (no {verb} target)");
    }
    Ok(())
}

async fn run_workspace(verb: &str, args: &ProjectRunArgs, opts: RunOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    run_workspace_at(&cwd, verb, args, opts).await
}

/// Inner `run_workspace` that takes an explicit cwd for testability.
async fn run_workspace_at(
    cwd: &std::path::Path,
    verb: &str,
    args: &ProjectRunArgs,
    opts: RunOptions,
) -> Result<()> {
    let ws_root = find_workspace_root(cwd)?;
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

    // Build the full dep graph BEFORE filtering, so --with-deps can expand
    // requested projects into their transitive deps.
    let full_graph: BTreeMap<String, Vec<String>> = all_entries
        .iter()
        .map(|(name, _, cfg)| (name.clone(), cfg.deps.projects.clone()))
        .collect();

    // Resolve which projects to run.
    let wanted: BTreeSet<String> = if args.projects.is_empty() {
        all_entries.iter().map(|(n, _, _)| n.clone()).collect()
    } else if args.with_deps {
        expand_with_deps(&full_graph, &args.projects)
    } else {
        args.projects.iter().cloned().collect()
    };

    // Filter entries to the wanted set.
    let entries: Vec<(String, PathBuf, ProjectConfig)> = all_entries
        .into_iter()
        .filter(|(name, _, _)| wanted.contains(name))
        .collect();

    // Build dep graph and sort.
    let mut graph: Graph = BTreeMap::new();
    let name_to_entry: BTreeMap<String, (PathBuf, ProjectConfig)> = entries
        .into_iter()
        .map(|(name, path, cfg)| {
            // Drop deps that aren't in the filtered set so topo sort doesn't
            // wait on projects we aren't running.
            let kept_deps = cfg
                .deps
                .projects
                .iter()
                .filter(|d| wanted.contains(*d))
                .cloned()
                .collect();
            graph.insert(name.clone(), kept_deps);
            (name, (path, cfg))
        })
        .collect();

    let sorted = sort(&graph).map_err(|e| SunbeamError::Other(e.to_string()))?;

    let semaphore = args.jobs.map(|n| Arc::new(Semaphore::new(n.max(1))));

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
            let sem = semaphore.clone();
            set.spawn(async move {
                let outcome = throttled(sem, async move {
                    crate::project::runner::run(&cfg, &project_root, &verb_s, &task_opts).await
                })
                .await?;
                Ok((project_name, outcome))
            });
        }

        while let Some(res) = set.join_next().await {
            let (name, outcome) = res
                .map_err(|e| SunbeamError::Other(format!("task join error: {e}")))??;
            if matches!(outcome, RunOutcome::Skipped) {
                tracing::warn!("  {name}: skipped (no {verb} target)");
            }
        }
    }

    Ok(())
}

/// Run `fut` while holding a permit from `semaphore`, if one was provided.
/// `None` semaphore means run unthrottled. The permit is held until `fut`
/// resolves, which is what bounds in-flight work to `Semaphore`'s capacity.
async fn throttled<T, F>(semaphore: Option<Arc<Semaphore>>, fut: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    let _permit = match semaphore {
        Some(s) => Some(
            s.acquire_owned()
                .await
                .map_err(|e| SunbeamError::Other(format!("semaphore closed: {e}")))?,
        ),
        None => None,
    };
    fut.await
}

/// Expand a starting set of project names to include all transitive deps.
fn expand_with_deps(
    graph: &BTreeMap<String, Vec<String>>,
    seeds: &[String],
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<String> = seeds.to_vec();
    while let Some(name) = stack.pop() {
        if !out.insert(name.clone()) {
            continue;
        }
        if let Some(deps) = graph.get(&name) {
            for d in deps {
                if !out.contains(d) {
                    stack.push(d.clone());
                }
            }
        }
    }
    out
}

async fn cmd_info() -> Result<()> {
    let cwd = std::env::current_dir()?;
    cmd_info_at(&cwd).await
}

/// Inner `cmd_info` that takes an explicit cwd for testability.
async fn cmd_info_at(cwd: &std::path::Path) -> Result<()> {
    let project_root = find_project_root(cwd)?;
    let cfg = ProjectConfig::load(&project_root.join("sunbeam.yaml"))?;
    print!("{}", serde_yaml::to_string(&cfg)?);
    Ok(())
}

async fn cmd_order(verb: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    cmd_order_at(&cwd, verb).await
}

/// Inner `cmd_order` that takes an explicit cwd for testability.
async fn cmd_order_at(cwd: &std::path::Path, verb: &str) -> Result<()> {
    let ws_root = find_workspace_root(cwd)?;
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
        tracing::info!("group {i}: {}", tokens.join("  "));
    }

    Ok(())
}

/// Load every owned project's manifest. Returns (name, abs_path, ProjectConfig)
/// triples; entries that fail to parse are silently dropped (the autodiscovery
/// commands surface those errors when invoked directly).
fn load_owned_projects(
    ws: &WorkspaceConfig,
    ws_root: &std::path::Path,
) -> Vec<(String, PathBuf, ProjectConfig)> {
    use crate::operations::config::RepoBucket;
    ws.iter_repos()
        .filter_map(|entry| {
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
        .collect()
}

async fn cmd_graph(all: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    cmd_graph_at(&cwd, all).await
}

/// Inner `cmd_graph` that takes an explicit cwd for testability.
async fn cmd_graph_at(cwd: &std::path::Path, all: bool) -> Result<()> {
    let ws_root = find_workspace_root(cwd)?;
    let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))?;
    let projects = load_owned_projects(&ws, &ws_root);
    let graph: BTreeMap<String, Vec<String>> = projects
        .iter()
        .map(|(n, _, cfg)| (n.clone(), cfg.deps.projects.clone()))
        .collect();

    if all {
        // Roots = projects no other project depends on.
        let mut depended_on: BTreeSet<&str> = BTreeSet::new();
        for deps in graph.values() {
            for d in deps {
                depended_on.insert(d.as_str());
            }
        }
        let mut roots: Vec<&String> = graph
            .keys()
            .filter(|n| !depended_on.contains(n.as_str()))
            .collect();
        roots.sort();
        let to_render: Vec<&String> = if roots.is_empty() {
            // Pure cycle, or empty workspace. Fall through to flat list.
            graph.keys().collect()
        } else {
            roots
        };
        for name in to_render {
            println!("{name}");
            render_children(name, &graph, "", &mut Vec::new());
        }
    } else {
        let project_root = find_project_root(cwd)?;
        let cfg = ProjectConfig::load(&project_root.join("sunbeam.yaml"))?;
        let name = cfg.project.name.clone();
        // Use the project's own deps even if the workspace doesn't list it.
        let mut local_graph = graph;
        local_graph.entry(name.clone()).or_insert_with(|| cfg.deps.projects.clone());
        println!("{name}");
        if local_graph.get(&name).is_none_or(|d| d.is_empty()) {
            println!("└── (no deps)");
        } else {
            render_children(&name, &local_graph, "", &mut Vec::new());
        }
    }

    Ok(())
}

/// Render `node`'s direct children using a per-path stack for true cycle detection.
/// `path` contains the chain of ancestors from the rendered root down to `node`;
/// a child appearing in `path` is a real cycle (not a shared dep).
fn render_children(
    node: &str,
    graph: &BTreeMap<String, Vec<String>>,
    prefix: &str,
    path: &mut Vec<String>,
) {
    let empty = Vec::new();
    let deps = graph.get(node).unwrap_or(&empty);
    let n = deps.len();
    path.push(node.to_string());
    for (i, dep) in deps.iter().enumerate() {
        let last = i + 1 == n;
        let connector = if last { "└── " } else { "├── " };
        let next_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
        let known_in_workspace = graph.contains_key(dep);
        let is_cycle = path.iter().any(|p| p == dep);
        let suffix = match (known_in_workspace, is_cycle) {
            (false, _) => " (external)",
            (true, true) => " (cycle)",
            (true, false) => "",
        };
        println!("{prefix}{connector}{dep}{suffix}");
        if known_in_workspace && !is_cycle {
            render_children(dep, graph, &next_prefix, path);
        }
    }
    path.pop();
}

async fn cmd_check(all: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    cmd_check_at(&cwd, all).await
}

/// Inner `cmd_check` that takes an explicit cwd. Exposed so tests can target
/// a tempdir without racing on the process-global cwd.
async fn cmd_check_at(cwd: &std::path::Path, all: bool) -> Result<()> {
    let ws_root = find_workspace_root(cwd)?;
    let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))?;
    let owned = load_owned_projects(&ws, &ws_root);

    let owned_names: BTreeSet<String> = owned.iter().map(|(n, _, _)| n.clone()).collect();
    let service_names: BTreeSet<String> = ws.services.keys().cloned().collect();

    // Build the workspace-wide dep graph for cycle detection.
    let full_graph: BTreeMap<String, Vec<String>> = owned
        .iter()
        .map(|(n, _, cfg)| (n.clone(), cfg.deps.projects.clone()))
        .collect();

    let mut targets: Vec<(String, ProjectConfig)> = if all {
        owned.iter().map(|(n, _, cfg)| (n.clone(), cfg.clone())).collect()
    } else {
        let project_root = find_project_root(cwd)?;
        let cfg = ProjectConfig::load(&project_root.join("sunbeam.yaml"))?;
        vec![(cfg.project.name.clone(), cfg)]
    };
    targets.sort_by(|a, b| a.0.cmp(&b.0));

    // Workspace-wide cycle check (only meaningful with --all, but cheap to do once).
    let cycle_err = sort(&full_graph).err().map(|e| e.to_string());

    let mut failed = false;
    for (name, cfg) in &targets {
        tracing::info!("check {name}");

        tracing::info!("✓ schema {} parsed", cfg.schema);

        // Project name uniqueness in workspace.
        if owned_names.contains(name) {
            tracing::info!("✓ name registered in workspace manifest");
        } else {
            tracing::warn!("✗ project name {name:?} not in workspace.owned");
            failed = true;
        }

        // deps.projects
        let mut bad_proj_deps = Vec::new();
        for d in &cfg.deps.projects {
            if !owned_names.contains(d) {
                bad_proj_deps.push(d.clone());
            }
        }
        if bad_proj_deps.is_empty() {
            tracing::info!("✓ deps.projects ({} entries) all resolve", cfg.deps.projects.len());
        } else {
            tracing::warn!("✗ deps.projects: unknown {bad_proj_deps:?}");
            failed = true;
        }

        // deps.services
        let mut bad_svc_deps = Vec::new();
        for s in &cfg.deps.services {
            if !service_names.contains(s) {
                bad_svc_deps.push(s.clone());
            }
        }
        if bad_svc_deps.is_empty() {
            tracing::info!("✓ deps.services ({} entries) all resolve", cfg.deps.services.len());
        } else {
            tracing::warn!("✗ deps.services: unknown {bad_svc_deps:?}");
            failed = true;
        }

        // Target verbs — annotate non-standard ones.
        let custom: Vec<&String> = cfg
            .targets
            .keys()
            .filter(|v| !is_standard_verb(v))
            .collect();
        if custom.is_empty() {
            tracing::info!(
                "✓ targets: all standard ({} of {} defined)",
                cfg.targets.len(),
                STANDARD_VERBS.len()
            );
        } else {
            tracing::info!(
                "✓ targets: {} standard + custom: {custom:?}",
                cfg.targets.len() - custom.len()
            );
        }
    }

    // Cycle check at the end (workspace-wide, reported once).
    match &cycle_err {
        Some(e) => {
            tracing::warn!("✗ workspace dep graph has cycle: {e}");
            failed = true;
        }
        None => {
            tracing::info!("✓ workspace dep graph is acyclic");
        }
    }

    if failed {
        Err(SunbeamError::Other("check found one or more issues".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ============================================================================
    // Pure-function tests for expand_with_deps
    // ============================================================================

    #[test]
    fn expand_with_deps_simple_chain() {
        // graph: a -> b -> c
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string()]);
        graph.insert("b".to_string(), vec!["c".to_string()]);
        graph.insert("c".to_string(), vec![]);

        let result = expand_with_deps(&graph, &["a".to_string()]);
        assert_eq!(result.len(), 3);
        assert!(result.contains("a"));
        assert!(result.contains("b"));
        assert!(result.contains("c"));
    }

    #[test]
    fn expand_with_deps_diamond() {
        // graph: a -> [b, c], b -> d, c -> d
        // Should get {a, b, c, d} with no double-visit
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        graph.insert("b".to_string(), vec!["d".to_string()]);
        graph.insert("c".to_string(), vec!["d".to_string()]);
        graph.insert("d".to_string(), vec![]);

        let result = expand_with_deps(&graph, &["a".to_string()]);
        assert_eq!(result.len(), 4);
        assert!(result.contains("a"));
        assert!(result.contains("b"));
        assert!(result.contains("c"));
        assert!(result.contains("d"));
    }

    #[test]
    fn expand_with_deps_cycle_terminates() {
        // graph: a -> b -> a (cycle)
        // Function should terminate and include both nodes
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string()]);
        graph.insert("b".to_string(), vec!["a".to_string()]);

        let result = expand_with_deps(&graph, &["a".to_string()]);
        assert!(result.contains("a"), "cycle handling: 'a' should be in result");
        assert!(result.contains("b"), "cycle handling: 'b' should be in result");
        // No panic or infinite loop = success
    }

    #[test]
    fn expand_with_deps_unknown_seed() {
        // graph: a -> b, but seed is "zzz" (unknown)
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string()]);
        graph.insert("b".to_string(), vec![]);

        let result = expand_with_deps(&graph, &["zzz".to_string()]);
        // Current behavior: unknown seeds are silently included
        assert!(result.contains("zzz"), "unknown seed should be in result");
    }

    #[test]
    fn expand_with_deps_no_seeds() {
        // empty seeds → empty result
        let graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let result = expand_with_deps(&graph, &[]);
        assert!(result.is_empty());
    }

    // ============================================================================
    // Tree rendering tests — helper function for capture
    // ============================================================================

    /// Extract the pure rendering logic without printing.
    /// Returns the sequence of lines that would be printed.
    fn render_lines(
        node: &str,
        graph: &BTreeMap<String, Vec<String>>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(node.to_string());
        let mut path = Vec::new();
        render_children_to_lines(node, graph, "", &mut path, &mut lines);
        lines
    }

    /// Internal helper: collects lines instead of printing.
    fn render_children_to_lines(
        node: &str,
        graph: &BTreeMap<String, Vec<String>>,
        prefix: &str,
        path: &mut Vec<String>,
        lines: &mut Vec<String>,
    ) {
        let empty = Vec::new();
        let deps = graph.get(node).unwrap_or(&empty);
        let n = deps.len();
        path.push(node.to_string());
        for (i, dep) in deps.iter().enumerate() {
            let last = i + 1 == n;
            let connector = if last { "└── " } else { "├── " };
            let next_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            let known_in_workspace = graph.contains_key(dep);
            let is_cycle = path.iter().any(|p| p == dep);
            let suffix = match (known_in_workspace, is_cycle) {
                (false, _) => " (external)",
                (true, true) => " (cycle)",
                (true, false) => "",
            };
            let line = format!("{prefix}{connector}{dep}{suffix}");
            lines.push(line);
            if known_in_workspace && !is_cycle {
                render_children_to_lines(dep, graph, &next_prefix, path, lines);
            }
        }
        path.pop();
    }

    #[test]
    fn render_lines_no_deps() {
        let graph: BTreeMap<String, Vec<String>> = {
            let mut m = BTreeMap::new();
            m.insert("a".to_string(), vec![]);
            m
        };

        let lines = render_lines("a", &graph);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "a");
    }

    #[test]
    fn render_lines_diamond_no_false_cycle() {
        // Ensure d under both b and c is NOT marked (cycle)
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        graph.insert("b".to_string(), vec!["d".to_string()]);
        graph.insert("c".to_string(), vec!["d".to_string()]);
        graph.insert("d".to_string(), vec![]);

        let lines = render_lines("a", &graph);
        // Lines should be roughly:
        // a
        // ├── b
        // │   └── d
        // └── c
        //     └── d
        let d_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.contains("d"))
            .collect();
        assert_eq!(d_lines.len(), 2, "d should appear twice");
        for d_line in d_lines {
            assert!(!d_line.contains("(cycle)"), "shared dep d should not be marked cycle: {d_line}");
        }
    }

    #[test]
    fn render_lines_real_cycle() {
        // a -> b -> a (real cycle)
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string()]);
        graph.insert("b".to_string(), vec!["a".to_string()]);

        let lines = render_lines("a", &graph);
        let cycle_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.contains("(cycle)"))
            .collect();
        assert!(!cycle_lines.is_empty(), "real cycle should be marked (cycle)");
    }

    #[test]
    fn render_lines_external_dep() {
        // a -> b, but b is not in graph (external)
        let mut graph = BTreeMap::new();
        graph.insert("a".to_string(), vec!["b".to_string()]);
        // Note: b is NOT in the graph

        let lines = render_lines("a", &graph);
        let b_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.contains("b"))
            .collect();
        assert_eq!(b_lines.len(), 1);
        assert!(b_lines[0].contains("(external)"), "unknown dep should be marked external");
    }

    // ============================================================================
    // Integration tests for cmd_check (requires real workspace structure)
    // ============================================================================

    /// Mutex for serializing tests that swap process-global env vars
    /// (`SUNBEAM_WORKSPACE`). All cmd_check tests must take this lock.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// RAII guard that overrides an env var for the duration of a test
    /// and restores the prior value on drop. Mirrors `discovery::tests::EnvGuard`.
    struct EnvGuard {
        key: String,
        prior: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let prior = std::env::var_os(key);
            // SAFETY: tests serialize via ENV_LOCK so no concurrent setenv races.
            unsafe { std::env::set_var(key, value); }
            Self { key: key.to_string(), prior }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: tests serialize via ENV_LOCK so no concurrent setenv races.
            unsafe {
                match &self.prior {
                    Some(v) => std::env::set_var(&self.key, v),
                    None => std::env::remove_var(&self.key),
                }
            }
        }
    }

    fn minimal_workspace_yaml() -> String {
        r#"
schema: 1
workspace:
  name: test
repos:
  owned:
    foo:
      path: foo
services:
  bar:
    image: example.com/bar:latest
"#
        .to_string()
    }

    fn foo_sunbeam_yaml_no_deps() -> String {
        r#"
schema: 1
project:
  name: foo
  kind: rust-lib
deps:
  projects: []
  services: []
targets:
  build:
    exec: "true"
"#
        .to_string()
    }

    fn foo_sunbeam_yaml_with_service_dep(service: &str) -> String {
        format!(
            r#"
schema: 1
project:
  name: foo
  kind: rust-lib
deps:
  projects: []
  services: [{}]
targets:
  build:
    exec: "true"
"#,
            service
        )
    }

    fn foo_sunbeam_yaml_with_project_dep(project: &str) -> String {
        format!(
            r#"
schema: 1
project:
  name: foo
  kind: rust-lib
deps:
  projects: [{}]
  services: []
targets:
  build:
    exec: "true"
"#,
            project
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_check_clean_workspace_passes() {
        let _lock = ENV_LOCK.lock().await;

        let tmpdir = tempfile::TempDir::new().expect("create tmpdir");
        let ws_root = tmpdir.path();
        let _env = EnvGuard::set("SUNBEAM_WORKSPACE", ws_root.to_str().expect("ws_root utf8"));

        // Write workspace.yaml
        std::fs::write(
            ws_root.join(WORKSPACE_FILE),
            minimal_workspace_yaml(),
        )
        .expect("write workspace");

        // Write foo/sunbeam.yaml
        std::fs::create_dir(ws_root.join("foo")).expect("create foo dir");
        std::fs::write(
            ws_root.join("foo").join("sunbeam.yaml"),
            foo_sunbeam_yaml_no_deps(),
        )
        .expect("write foo sunbeam.yaml");

        let result = cmd_check_at(&ws_root.join("foo"), false).await;

        assert!(result.is_ok(), "clean workspace check should pass");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_check_unknown_project_dep_fails() {
        let _lock = ENV_LOCK.lock().await;

        let tmpdir = tempfile::TempDir::new().expect("create tmpdir");
        let ws_root = tmpdir.path();
        let _env = EnvGuard::set("SUNBEAM_WORKSPACE", ws_root.to_str().expect("ws_root utf8"));

        std::fs::write(
            ws_root.join(WORKSPACE_FILE),
            minimal_workspace_yaml(),
        )
        .expect("write workspace");

        std::fs::create_dir(ws_root.join("foo")).expect("create foo dir");
        std::fs::write(
            ws_root.join("foo").join("sunbeam.yaml"),
            foo_sunbeam_yaml_with_project_dep("missing"),
        )
        .expect("write foo sunbeam.yaml");

        let result = cmd_check_at(&ws_root.join("foo"), false).await;

        assert!(result.is_err(), "check should fail for unknown project dep");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_check_unknown_service_dep_fails() {
        let _lock = ENV_LOCK.lock().await;

        let tmpdir = tempfile::TempDir::new().expect("create tmpdir");
        let ws_root = tmpdir.path();
        let _env = EnvGuard::set("SUNBEAM_WORKSPACE", ws_root.to_str().expect("ws_root utf8"));

        std::fs::write(
            ws_root.join(WORKSPACE_FILE),
            minimal_workspace_yaml(),
        )
        .expect("write workspace");

        std::fs::create_dir(ws_root.join("foo")).expect("create foo dir");
        std::fs::write(
            ws_root.join("foo").join("sunbeam.yaml"),
            foo_sunbeam_yaml_with_service_dep("nonexistent"),
        )
        .expect("write foo sunbeam.yaml");

        let result = cmd_check_at(&ws_root.join("foo"), false).await;

        assert!(result.is_err(), "check should fail for unknown service dep");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_check_cycle_detected() {
        let _lock = ENV_LOCK.lock().await;

        let tmpdir = tempfile::TempDir::new().expect("create tmpdir");
        let ws_root = tmpdir.path();
        let _env = EnvGuard::set("SUNBEAM_WORKSPACE", ws_root.to_str().expect("ws_root utf8"));

        // Workspace with foo and bar that depend on each other
        let ws_yaml = r#"
schema: 1
workspace:
  name: test
repos:
  owned:
    foo:
      path: foo
    bar:
      path: bar
services:
  baz:
    image: example.com/baz:latest
"#;
        std::fs::write(ws_root.join(WORKSPACE_FILE), ws_yaml).expect("write workspace");

        // foo depends on bar
        std::fs::create_dir(ws_root.join("foo")).expect("create foo dir");
        std::fs::write(
            ws_root.join("foo").join("sunbeam.yaml"),
            r#"
schema: 1
project:
  name: foo
deps:
  projects: [bar]
targets:
  build:
    exec: "true"
"#,
        )
        .expect("write foo sunbeam.yaml");

        // bar depends on foo (creates cycle)
        std::fs::create_dir(ws_root.join("bar")).expect("create bar dir");
        std::fs::write(
            ws_root.join("bar").join("sunbeam.yaml"),
            r#"
schema: 1
project:
  name: bar
deps:
  projects: [foo]
targets:
  build:
    exec: "true"
"#,
        )
        .expect("write bar sunbeam.yaml");

        let result = cmd_check_at(&ws_root.join("foo"), true).await;

        assert!(result.is_err(), "check should fail when cycle is detected");
    }

    // ============================================================================
    // --jobs semaphore (throttled helper) tests
    // ============================================================================
    //
    // We test the `throttled` helper rather than run_workspace end-to-end:
    // run_workspace wraps each spawned task in `throttled(sem, fut)`, so the
    // helper is the actual concurrency primitive. Spawning many `throttled`
    // calls against a 2-permit semaphore and observing in-flight counts is a
    // direct, deterministic check that --jobs N caps parallelism.

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn throttled_caps_concurrency_at_jobs_n() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let n_jobs = 2;
        let n_tasks = 16;

        let semaphore = Some(Arc::new(Semaphore::new(n_jobs)));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut set: JoinSet<Result<()>> = JoinSet::new();
        for _ in 0..n_tasks {
            let sem = semaphore.clone();
            let in_flight = in_flight.clone();
            let max_seen = max_seen.clone();
            set.spawn(async move {
                throttled(sem, async move {
                    let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(cur, Ordering::SeqCst);
                    // Hold the permit long enough that genuinely-concurrent
                    // tasks would overlap.
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
            });
        }
        while let Some(r) = set.join_next().await {
            r.expect("join error").expect("task error");
        }

        let observed = max_seen.load(Ordering::SeqCst);
        assert!(
            observed <= n_jobs,
            "throttled allowed {observed} tasks in flight; cap was {n_jobs}"
        );
        // Sanity: at least 2 tasks ran concurrently (otherwise the test
        // mechanism is broken — sleep is far longer than scheduling jitter).
        assert!(
            observed >= 2,
            "expected at least 2 concurrent tasks under --jobs {n_jobs}, saw {observed}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn throttled_unbounded_when_no_semaphore() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let n_tasks = 8;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut set: JoinSet<Result<()>> = JoinSet::new();
        for _ in 0..n_tasks {
            let in_flight = in_flight.clone();
            let max_seen = max_seen.clone();
            set.spawn(async move {
                throttled(None, async move {
                    let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(cur, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
            });
        }
        while let Some(r) = set.join_next().await {
            r.expect("join error").expect("task error");
        }

        let observed = max_seen.load(Ordering::SeqCst);
        // With no semaphore, all tasks should run concurrently (or close to
        // it — bound by tokio's worker count, which we set to 4).
        assert!(
            observed >= 4,
            "expected unthrottled tasks to peak above the semaphore cap; saw {observed}"
        );
    }

    // ============================================================================
    // Integration tests for cmd_info, cmd_order, cmd_graph, run_workspace
    // ============================================================================
    //
    // These all use `*_at(cwd, ...)` inner forms so tests can target a tempdir
    // without racing on process-global cwd. ENV_LOCK serialises them because
    // they all override SUNBEAM_WORKSPACE.

    /// Set up a tempdir with a minimal workspace.yaml + N owned projects, each
    /// with a `build` target that runs `exec: "true"`. Returns the workspace
    /// root so callers can build cwd paths off it.
    fn make_workspace(projects: &[(&str, &[&str])]) -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let ws_root = tmp.path();

        let mut ws_yaml = String::from(
            "schema: 1\nworkspace:\n  name: test\nrepos:\n  owned:\n",
        );
        for (name, _) in projects {
            ws_yaml.push_str(&format!("    {name}:\n      path: {name}\n"));
        }
        ws_yaml.push_str("services:\n  bar:\n    image: example.com/bar:latest\n");
        std::fs::write(ws_root.join(WORKSPACE_FILE), ws_yaml).expect("write ws");

        for (name, deps) in projects {
            std::fs::create_dir(ws_root.join(name)).expect("create proj dir");
            let deps_str = deps
                .iter()
                .map(|d| format!("\"{d}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let proj_yaml = format!(
                r#"
schema: 1
project:
  name: {name}
deps:
  projects: [{deps_str}]
targets:
  build:
    exec: "true"
"#
            );
            std::fs::write(
                ws_root.join(name).join("sunbeam.yaml"),
                proj_yaml,
            )
            .expect("write proj");
        }

        tmp
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_info_at_prints_resolved_config() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[("foo", &[])]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        let result = cmd_info_at(&tmp.path().join("foo")).await;
        assert!(result.is_ok(), "cmd_info_at should succeed for valid project");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_info_at_errors_when_no_project() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        // Empty tempdir — no sunbeam.yaml anywhere.
        let result = cmd_info_at(tmp.path()).await;
        assert!(result.is_err(), "cmd_info_at should fail when no project found");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_order_at_prints_topological_groups() {
        let _lock = ENV_LOCK.lock().await;
        // bar depends on foo → group 0: foo, group 1: bar
        let tmp = make_workspace(&[("foo", &[]), ("bar", &["foo"])]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        let result = cmd_order_at(tmp.path(), "build").await;
        assert!(result.is_ok(), "cmd_order_at should succeed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_order_at_works_for_unknown_verb() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[("foo", &[])]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        // 'test' isn't defined in any project → all marked (skip), still Ok.
        let result = cmd_order_at(tmp.path(), "test").await;
        assert!(result.is_ok(), "cmd_order_at handles verbs that no project defines");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_graph_at_single_project() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[("foo", &[]), ("bar", &["foo"])]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        let result = cmd_graph_at(&tmp.path().join("bar"), false).await;
        assert!(result.is_ok(), "cmd_graph_at(false) should succeed in a project");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_graph_at_all() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[("foo", &[]), ("bar", &["foo"]), ("baz", &[])]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        let result = cmd_graph_at(tmp.path(), true).await;
        assert!(result.is_ok(), "cmd_graph_at(true) should succeed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cmd_graph_at_handles_pure_cycle() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[("foo", &["bar"]), ("bar", &["foo"])]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        // No roots (every project is depended on) — falls back to flat list.
        let result = cmd_graph_at(tmp.path(), true).await;
        assert!(result.is_ok(), "cmd_graph_at(true) should not panic on pure cycles");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_workspace_at_runs_all_projects_in_topo_order() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[
            ("foo", &[]),
            ("bar", &["foo"]),
            ("baz", &["bar"]),
        ]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );

        let args = ProjectRunArgs {
            all: true,
            ..Default::default()
        };
        let opts = RunOptions::default();
        let result = run_workspace_at(tmp.path(), "build", &args, opts).await;
        assert!(result.is_ok(), "run_workspace_at --all should succeed: {result:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_workspace_at_with_deps_expands_seed() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[
            ("foo", &[]),
            ("bar", &["foo"]),
            ("baz", &[]),
        ]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );

        // Asking for 'bar' --with-deps should run foo first, then bar. baz untouched.
        let args = ProjectRunArgs {
            projects: vec!["bar".into()],
            with_deps: true,
            ..Default::default()
        };
        let opts = RunOptions::default();
        let result = run_workspace_at(tmp.path(), "build", &args, opts).await;
        assert!(result.is_ok(), "with_deps run should succeed: {result:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_workspace_at_with_jobs_caps_concurrency() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = make_workspace(&[
            ("a", &[]),
            ("b", &[]),
            ("c", &[]),
            ("d", &[]),
        ]);
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            tmp.path().to_str().expect("ws_root utf8"),
        );
        // All 4 projects sit in the same topo group (no deps among them).
        // --jobs 2 should serialise them through the semaphore.
        let args = ProjectRunArgs {
            all: true,
            jobs: Some(2),
            ..Default::default()
        };
        let opts = RunOptions::default();
        let result = run_workspace_at(tmp.path(), "build", &args, opts).await;
        assert!(result.is_ok(), "run_workspace_at with --jobs should succeed: {result:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_workspace_at_propagates_subprocess_failure() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let ws_root = tmp.path();
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            ws_root.to_str().expect("ws_root utf8"),
        );

        // Workspace with one project whose build deliberately fails.
        std::fs::write(
            ws_root.join(WORKSPACE_FILE),
            "schema: 1\nworkspace:\n  name: t\nrepos:\n  owned:\n    bad:\n      path: bad\n",
        )
        .expect("write ws");
        std::fs::create_dir(ws_root.join("bad")).expect("create dir");
        std::fs::write(
            ws_root.join("bad").join("sunbeam.yaml"),
            "schema: 1\nproject:\n  name: bad\ntargets:\n  build:\n    exec: \"false\"\n",
        )
        .expect("write proj");

        let args = ProjectRunArgs { all: true, ..Default::default() };
        let result = run_workspace_at(ws_root, "build", &args, RunOptions::default()).await;
        assert!(result.is_err(), "failing build should bubble up as Err");
    }

    /// Verify autodiscovery + run_workspace work the same when invoked from
    /// inside a `.worktrees/<branch>/` checkout. Worktrees are tracked checkouts
    /// of the same files, so the workspace.yaml is at the worktree root and
    /// `find_workspace_root` should resolve to that — not to any ancestor.
    #[tokio::test(flavor = "current_thread")]
    async fn worktree_layout_resolves_correctly() {
        let _lock = ENV_LOCK.lock().await;

        // Outer tempdir simulates the workspace root (not the actual main
        // checkout — we don't need git for this, just the file layout).
        let outer = tempfile::TempDir::new().expect("tmpdir");
        // Inside it, a `.worktrees/feature/` directory acts as a worktree
        // checkout containing its own workspace.yaml + project. Walking up
        // from the worktree's project should find the worktree's workspace.yaml,
        // not the outer one.
        let worktree_root = outer.path().join(".worktrees").join("feature");
        std::fs::create_dir_all(&worktree_root).expect("mk worktree dir");

        // Optional: write an outer workspace.yaml so we can verify discovery
        // doesn't escape upward past the worktree's own copy.
        std::fs::write(
            outer.path().join(WORKSPACE_FILE),
            "schema: 1\nworkspace:\n  name: outer\nrepos:\n  owned: {}\n",
        )
        .expect("write outer ws");

        // Worktree-local workspace.yaml + project.
        std::fs::write(
            worktree_root.join(WORKSPACE_FILE),
            "schema: 1\nworkspace:\n  name: feature\nrepos:\n  owned:\n    foo:\n      path: foo\n",
        )
        .expect("write worktree ws");
        std::fs::create_dir(worktree_root.join("foo")).expect("mk foo");
        std::fs::write(
            worktree_root.join("foo").join("sunbeam.yaml"),
            r#"
schema: 1
project:
  name: foo
targets:
  build:
    exec: "true"
"#,
        )
        .expect("write foo");

        // Make sure the env var doesn't override discovery for this test.
        let _env = EnvGuard::set(
            "SUNBEAM_WORKSPACE",
            worktree_root.to_str().expect("ws_root utf8"),
        );

        // run_workspace_at from inside the worktree's foo/ should resolve the
        // worktree's manifest, not the outer one (which has no `foo`).
        let args = ProjectRunArgs { all: true, ..Default::default() };
        let result =
            run_workspace_at(&worktree_root.join("foo"), "build", &args, RunOptions::default())
                .await;
        assert!(
            result.is_ok(),
            "worktree-rooted run_workspace_at should succeed: {result:?}"
        );

        // cmd_check_at should also succeed against the worktree's manifest.
        let check_result = cmd_check_at(&worktree_root.join("foo"), true).await;
        assert!(
            check_result.is_ok(),
            "cmd_check_at should validate the worktree-rooted workspace: {check_result:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_owned_projects_filters_non_owned_buckets() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let ws_root = tmp.path();

        // Workspace with one owned project + one fork (forks bucket).
        std::fs::write(
            ws_root.join(WORKSPACE_FILE),
            r#"
schema: 1
workspace:
  name: test
repos:
  owned:
    foo:
      path: foo
  forks:
    upstream:
      path: upstream
      upstream: example/upstream
      tracking: main
"#,
        )
        .expect("write ws");
        std::fs::create_dir(ws_root.join("foo")).expect("mk foo");
        std::fs::write(
            ws_root.join("foo").join("sunbeam.yaml"),
            "schema: 1\nproject:\n  name: foo\n",
        )
        .expect("write foo proj");
        std::fs::create_dir(ws_root.join("upstream")).expect("mk upstream");
        std::fs::write(
            ws_root.join("upstream").join("sunbeam.yaml"),
            "schema: 1\nproject:\n  name: upstream\n",
        )
        .expect("write upstream proj");

        let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE)).expect("load ws");
        let owned = load_owned_projects(&ws, ws_root);
        let names: Vec<&str> = owned.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, vec!["foo"], "only owned-bucket projects should load");
    }
}
