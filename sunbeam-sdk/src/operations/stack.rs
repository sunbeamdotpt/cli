//! Stack snapshot operations: pin, apply, diff, list.
//!
//! A stack is a named map of `project_name → git_sha` stored under
//! `stacks:` in `sunbeam.workspace.yaml`. These functions read and
//! mutate `WorkspaceConfig` in memory; callers persist via `save`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Result, ResultExt, SunbeamError};
use crate::operations::{Stack, WorkspaceConfig};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
/// Stacksummary.
pub struct StackSummary {
    /// Name.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// Project count.
    pub project_count: usize,
    /// Pinned at.
    pub pinned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Stackdiffentry.
pub struct StackDiffEntry {
    /// Project.
    pub project: String,
    /// Left.
    pub left: Option<String>,
    /// Right.
    pub right: Option<String>,
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// List.
pub fn list(ws: &WorkspaceConfig) -> Vec<StackSummary> {
    ws.stacks
        .iter()
        .map(|(name, stack)| StackSummary {
            name: name.clone(),
            description: stack.description.clone(),
            project_count: stack.projects.len(),
            pinned_at: stack.pinned_at.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// pin
// ---------------------------------------------------------------------------

/// Pin.
pub fn pin(
    ws: &mut WorkspaceConfig,
    workspace_root: &Path,
    stack_name: &str,
    description: Option<&str>,
    projects: &[String],
) -> Result<()> {
    let project_names: Vec<String> = if projects.is_empty() {
        ws.repos.owned.keys().cloned().collect()
    } else {
        projects.to_vec()
    };

    let mut pinned: BTreeMap<String, String> = BTreeMap::new();

    for name in &project_names {
        let entry = ws
            .find_repo(name)
            .with_ctx(|| format!("unknown project {name:?}: not found in workspace repos"))?;
        let abs_path = workspace_root.join(&entry.repo.path);
        let abs_str = abs_path.to_string_lossy();

        let out = std::process::Command::new("git")
            .args(["-C", &abs_str, "rev-parse", "HEAD"])
            .output()
            .with_ctx(|| format!("spawning git rev-parse HEAD in {abs_str}"))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(SunbeamError::tool(
                "git",
                format!("rev-parse HEAD failed in {abs_str}: {stderr}"),
            ));
        }

        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        pinned.insert(name.clone(), sha);
    }

    let stack = Stack {
        description: description.map(String::from),
        projects: pinned,
        pinned_at: Some(chrono::Utc::now().to_rfc3339()),
        extra: BTreeMap::new(),
    };

    ws.stacks.insert(stack_name.to_string(), stack);
    Ok(())
}

// ---------------------------------------------------------------------------
// save
// ---------------------------------------------------------------------------

/// Save.
pub fn save(ws: &WorkspaceConfig, workspace_root: &Path) -> Result<()> {
    let yaml = serde_yaml::to_string(ws)?;
    let dest = workspace_root.join("sunbeam.workspace.yaml");
    let tmp = workspace_root.join(".sunbeam.workspace.yaml.tmp");

    std::fs::write(&tmp, yaml).with_ctx(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &dest).with_ctx(|| format!("renaming tmp to {}", dest.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// apply
// ---------------------------------------------------------------------------

/// Apply.
#[tracing::instrument]
pub async fn apply(ws: &WorkspaceConfig, workspace_root: &Path, stack_name: &str) -> Result<()> {
    tracing::info!("stack apply");
    let stack = ws
        .stacks
        .get(stack_name)
        .with_ctx(|| format!("stack {stack_name:?} not found in workspace"))?;

    for (project, sha) in &stack.projects {
        let entry = ws
            .find_repo(project)
            .with_ctx(|| format!("stack {stack_name:?} references unknown project {project:?}"))?;
        let abs_path = workspace_root.join(&entry.repo.path);
        let abs_str = abs_path.to_string_lossy().into_owned();

        let fetch_status = tokio::process::Command::new("git")
            .args(["-C", &abs_str, "fetch"])
            .status()
            .await
            .with_ctx(|| format!("spawning git fetch in {abs_str}"))?;

        if !fetch_status.success() {
            return Err(SunbeamError::tool(
                "git",
                format!("fetch failed in {abs_str}"),
            ));
        }

        let checkout_status = tokio::process::Command::new("git")
            .args(["-C", &abs_str, "checkout", sha])
            .status()
            .await
            .with_ctx(|| format!("spawning git checkout {sha} in {abs_str}"))?;

        if !checkout_status.success() {
            return Err(SunbeamError::tool(
                "git",
                format!("checkout {sha} failed in {abs_str}"),
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

/// Diff.
pub fn diff(
    ws: &WorkspaceConfig,
    workspace_root: &Path,
    left: &str,
    right: Option<&str>,
) -> Result<Vec<StackDiffEntry>> {
    let left_stack = ws
        .stacks
        .get(left)
        .with_ctx(|| format!("stack {left:?} not found in workspace"))?;

    let mut entries: Vec<StackDiffEntry> = Vec::new();

    match right {
        Some(right_name) => {
            let right_stack = ws
                .stacks
                .get(right_name)
                .with_ctx(|| format!("stack {right_name:?} not found in workspace"))?;

            let all_projects: std::collections::BTreeSet<&String> = left_stack
                .projects
                .keys()
                .chain(right_stack.projects.keys())
                .collect();

            for project in all_projects {
                let l = left_stack.projects.get(project).cloned();
                let r = right_stack.projects.get(project).cloned();
                if l != r {
                    entries.push(StackDiffEntry {
                        project: project.clone(),
                        left: l,
                        right: r,
                    });
                }
            }
        }
        None => {
            for (project, stack_sha) in &left_stack.projects {
                let head_sha = match ws.find_repo(project) {
                    None => None,
                    Some(entry) => {
                        let abs_path = workspace_root.join(&entry.repo.path);
                        let abs_str = abs_path.to_string_lossy();
                        match std::process::Command::new("git")
                            .args(["-C", abs_str.as_ref(), "rev-parse", "HEAD"])
                            .output()
                        {
                            Err(_) => None,
                            Ok(out) if !out.status.success() => None,
                            Ok(out) => {
                                let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                if sha.is_empty() { None } else { Some(sha) }
                            }
                        }
                    }
                };

                if head_sha.as_deref() != Some(stack_sha.as_str()) {
                    entries.push(StackDiffEntry {
                        project: project.clone(),
                        left: Some(stack_sha.clone()),
                        right: head_sha,
                    });
                }
            }
        }
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::config::{Repo, Repos, WorkspaceMeta};

    fn minimal_ws() -> WorkspaceConfig {
        WorkspaceConfig {
            schema: 1,
            workspace: WorkspaceMeta {
                name: "test".into(),
                root: ".".into(),
            },
            repos: Repos::default(),
            services: BTreeMap::new(),
            volumes: BTreeMap::new(),
            stacks: BTreeMap::new(),
        }
    }

    fn ws_with_repo(name: &str, path: &str) -> WorkspaceConfig {
        let mut ws = minimal_ws();
        ws.repos.owned.insert(
            name.to_string(),
            Repo {
                path: path.to_string(),
                kind: None,
                upstream: None,
                tracking: None,
                extra: BTreeMap::new(),
            },
        );
        ws
    }

    #[test]
    fn list_is_empty_by_default() {
        let ws = minimal_ws();
        assert!(list(&ws).is_empty());
    }

    #[test]
    fn pin_errors_on_unknown_project() {
        let mut ws = minimal_ws();
        let tmp = std::env::temp_dir();
        let err = pin(&mut ws, &tmp, "s1", None, &["ghost".to_string()]).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "requires git in PATH"]
    fn pin_populates_stack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // Init a git repo and create an empty commit.
        std::process::Command::new("git")
            .args(["-C", &path.to_string_lossy(), "init"])
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-C",
                &path.to_string_lossy(),
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-m",
                "x",
            ])
            .status()
            .unwrap();

        let mut ws = ws_with_repo("myrepo", path.to_str().unwrap());
        // Use an absolute path as workspace_root so join works correctly.
        pin(
            &mut ws,
            Path::new("/"),
            "snap",
            Some("desc"),
            &["myrepo".to_string()],
        )
        .unwrap();

        let stack = ws.stacks.get("snap").unwrap();
        assert_eq!(stack.description.as_deref(), Some("desc"));
        assert_eq!(stack.projects.len(), 1);
        let sha = stack.projects.get("myrepo").unwrap();
        assert_eq!(sha.len(), 40, "expected a 40-char SHA, got {sha:?}");
        assert!(stack.pinned_at.is_some());
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "requires git in PATH"]
    fn save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["-C", &repo_dir.path().to_string_lossy(), "init"])
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-C",
                &repo_dir.path().to_string_lossy(),
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .status()
            .unwrap();

        let mut ws = ws_with_repo("sol", repo_dir.path().to_str().unwrap());

        pin(&mut ws, Path::new("/"), "rc1", None, &["sol".to_string()]).unwrap();
        save(&ws, dir.path()).unwrap();

        let manifest = dir.path().join("sunbeam.workspace.yaml");
        let loaded = WorkspaceConfig::load(&manifest).unwrap();

        assert_eq!(ws.stacks["rc1"].projects, loaded.stacks["rc1"].projects,);
    }

    #[test]
    fn diff_two_stacks() {
        let mut ws = minimal_ws();
        ws.repos.owned.insert(
            "sol".to_string(),
            Repo {
                path: "sol".into(),
                kind: None,
                upstream: None,
                tracking: None,
                extra: BTreeMap::new(),
            },
        );
        ws.repos.owned.insert(
            "wfe".to_string(),
            Repo {
                path: "wfe".into(),
                kind: None,
                upstream: None,
                tracking: None,
                extra: BTreeMap::new(),
            },
        );

        let mut left_projects = BTreeMap::new();
        left_projects.insert("sol".to_string(), "aaa".to_string());
        left_projects.insert("wfe".to_string(), "shared".to_string());

        let mut right_projects = BTreeMap::new();
        right_projects.insert("sol".to_string(), "bbb".to_string());
        right_projects.insert("wfe".to_string(), "shared".to_string());

        ws.stacks.insert(
            "left".to_string(),
            Stack {
                description: None,
                projects: left_projects,
                pinned_at: None,
                extra: BTreeMap::new(),
            },
        );
        ws.stacks.insert(
            "right".to_string(),
            Stack {
                description: None,
                projects: right_projects,
                pinned_at: None,
                extra: BTreeMap::new(),
            },
        );

        let changes = diff(&ws, Path::new("/"), "left", Some("right")).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].project, "sol");
        assert_eq!(changes[0].left.as_deref(), Some("aaa"));
        assert_eq!(changes[0].right.as_deref(), Some("bbb"));
    }
}
