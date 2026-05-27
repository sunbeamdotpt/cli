//! Build project container images via the manifest ecosystem.
//!
//! Discovers owned projects from `sunbeam.workspace.yaml`, filters to those
//! with a non-skip `package` target, topologically sorts by `deps.projects`,
//! and runs each `package` target with `SUNBEAM_REGISTRY` injected.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use wfe_core::models::ExecutionResult;
use wfe_core::traits::{StepBody, StepExecutionContext};

use crate::discovery::{WORKSPACE_FILE, find_workspace_root};
use crate::operations::config::{RepoBucket, WorkspaceConfig};

use crate::project::config::ProjectConfig;
use crate::project::runner::{RunOptions, RunOutcome};
use crate::topo::{Graph, sort};
use crate::workflows::StepContext;

fn step_err(msg: impl Into<String>) -> wfe_core::WfeError {
    wfe_core::WfeError::StepExecution(msg.into())
}

/// Discover owned workspace projects and run `package` targets for any that
/// declare them.
#[derive(Default)]
pub struct BuildProjectImages;

#[async_trait::async_trait]
impl StepBody for BuildProjectImages {
    async fn run(&mut self, ctx: &StepExecutionContext<'_>) -> wfe_core::Result<ExecutionResult> {
        let lima_skip: Vec<String> = ctx
            .workflow
            .data
            .get("lima_skip_namespaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if lima_skip.contains(&"oci".to_string()) {
            tracing::info!("Skipping project image builds (Lima VM — no OCI registry)");
            return Ok(ExecutionResult::next());
        }

        let step_ctx: StepContext =
            serde_json::from_value(ctx.workflow.data.get("__ctx").cloned().unwrap_or_default())
                .map_err(|e| step_err(e.to_string()))?;

        // Prefer the workflow data domain (updated dynamically by EnsureCilium
        // from the live cluster/Lima IP) over the static step_ctx.domain which
        // is a snapshot from config at workflow start time.
        let domain = ctx.workflow
            .data
            .get("domain")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&step_ctx.domain)
            .to_string();

        tracing::info!("Building project images...");

        // 1. Discover workspace root from current directory.
        let cwd = std::env::current_dir()
            .map_err(|e| step_err(format!("Failed to get current directory: {e}")))?;
        let ws_root = find_workspace_root(&cwd)
            .map_err(|e| step_err(format!("Failed to find workspace root: {e}")))?;

        // 2. Load workspace config.
        let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))
            .map_err(|e| step_err(format!("Failed to load workspace config: {e}")))?;

        // 3. Collect owned projects that have a package target.
        let mut entries: Vec<(String, PathBuf, ProjectConfig)> = Vec::new();
        for repo_entry in ws.iter_repos() {
            // Owned projects and forks that declare package targets are both
            // candidates for image builds.
            if repo_entry.bucket != RepoBucket::Owned && repo_entry.bucket != RepoBucket::Forks {
                continue;
            }
            let abs = ws_root.join(&repo_entry.repo.path);
            let manifest = abs.join("sunbeam.yaml");
            if !manifest.exists() {
                continue;
            }
            match ProjectConfig::load(&manifest) {
                Ok(cfg) => {
                    if cfg.has_target("package") {
                        entries.push((repo_entry.name.to_string(), abs, cfg));
                    }
                }
                Err(e) => {
                    return Err(step_err(format!(
                        "Failed to load {}: {e}",
                        manifest.display()
                    )));
                }
            }
        }

        if entries.is_empty() {
            tracing::info!("No projects with package targets — skipping image build.");
            return Ok(ExecutionResult::next());
        }

        // 4. Build dep graph and topologically sort.
        let mut graph: Graph = BTreeMap::new();
        let name_to_entry: BTreeMap<String, (PathBuf, ProjectConfig)> = entries
            .into_iter()
            .map(|(name, path, cfg)| {
                graph.insert(name.clone(), cfg.deps.projects.clone());
                (name, (path, cfg))
            })
            .collect();

        let sorted = sort(&graph).map_err(|e| step_err(e.to_string()))?;

        // 5. Run package targets in order.
        let mut extra_env = BTreeMap::new();
        if !domain.is_empty() {
            extra_env.insert("SUNBEAM_REGISTRY".to_string(), domain.clone());
        }
        let opts = RunOptions {
            extra_env,
            verbose: false,
            dry_run: false,
        };

        // Build sequentially within each topological group to avoid exhausting
        // disk on fresh installs with small Docker VMs. Parallel builds spike
        // peak disk usage; sequential keeps it bounded to one project at a time.
        for group in &sorted.0 {
            for project_name in group {
                let Some((project_root, cfg)) = name_to_entry.get(project_name) else {
                    continue;
                };
                let project_root = project_root.clone();
                let cfg = cfg.clone();
                let opts = opts.clone();
                let project_name = project_name.clone();

                tracing::info!("Building {project_name}...");
                match crate::project::runner::run(&cfg, &project_root, "package", &opts).await {
                    Ok(RunOutcome::Ran) => tracing::info!("Built {project_name}"),
                    Ok(RunOutcome::Skipped) => {
                        tracing::info!("{project_name}: skipped (no package target)");
                    }
                    Err(e) => {
                        tracing::warn!("Image build failed for {project_name}: {e}");
                        // With strict failures enabled, we propagate the error so
                        // the workflow terminates. Remove this return if you prefer
                        // best-effort builds.
                        return Err(step_err(format!(
                            "Failed to build {project_name}: {e}"
                        )));
                    }
                }
            }
        }

        tracing::info!("Project images build pass complete.");
        Ok(ExecutionResult::next())
    }
}
