//! Version control commands (git) with monorepo awareness.

mod branch;
mod clone;
mod commit;
mod fetch;
mod log;
mod pull;
mod push;
mod status;

use std::path::{Path, PathBuf};

use crate::cli::VcsAction;
use crate::discovery::{WORKSPACE_FILE, find_workspace_root_opt};
use crate::error::{Result, SunbeamError};
use crate::operations::config::WorkspaceConfig;

/// Common args for vcs subcommands.
#[derive(clap::Args, Debug, Clone)]
pub struct VcsArgs {
    /// Target a specific repo by name (workspace or repo-rs source root).
    #[arg(long, short = 'r')]
    pub repo: Option<String>,
    /// Run across all workspace repos.
    #[arg(long, short = 'a')]
    pub all: bool,
}

/// Resolved target for a vcs command.
#[derive(Debug, Clone)]
pub struct RepoTarget {
    pub name: String,
    pub path: PathBuf,
}

/// Top-level dispatch.
pub async fn dispatch(action: VcsAction) -> Result<()> {
    match action {
        VcsAction::Status { args } => status::cmd_status(args).await,
        VcsAction::Log {
            args,
            oneline,
            limit,
        } => log::cmd_log(args, oneline, limit).await,
        VcsAction::Branch {
            args,
            list,
            create,
            delete,
        } => branch::cmd_branch(args, list, create, delete).await,
        VcsAction::Commit { args, message, all } => commit::cmd_commit(args, message, all).await,
        VcsAction::Push {
            args,
            remote,
            set_upstream,
        } => push::cmd_push(args, remote, set_upstream).await,
        VcsAction::Fetch { args, remote } => fetch::cmd_fetch(args, remote).await,
        VcsAction::Pull { args, remote } => pull::cmd_pull(args, remote).await,
        VcsAction::Clone { url, name } => clone::cmd_clone(url, name).await,
    }
}

/// Resolve one or more repo targets from user args.
pub fn resolve_targets(args: &VcsArgs) -> Result<Vec<RepoTarget>> {
    if args.all {
        return resolve_workspace_all();
    }

    if let Some(name) = &args.repo {
        let target = resolve_by_name(name)?;
        return Ok(vec![target]);
    }

    let cwd = std::env::current_dir()?;
    if is_git_repo(&cwd) {
        let name = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(".")
            .to_string();
        return Ok(vec![RepoTarget { name, path: cwd }]);
    }

    Err(SunbeamError::Config(
        "not inside a git repo and no --repo specified".into(),
    ))
}

fn resolve_workspace_all() -> Result<Vec<RepoTarget>> {
    let cwd = std::env::current_dir()?;
    let ws_root = match find_workspace_root_opt(&cwd)? {
        Some(r) => r,
        None => {
            return Err(SunbeamError::Config(
                "--all requires a sunbeam workspace".into(),
            ));
        }
    };
    let ws = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE))?;
    let mut targets = Vec::new();
    for entry in ws.iter_repos() {
        let path = ws_root.join(&entry.repo.path);
        if !is_git_repo(&path) {
            continue;
        }
        targets.push(RepoTarget {
            name: entry.name.to_string(),
            path,
        });
    }
    Ok(targets)
}

fn resolve_by_name(name: &str) -> Result<RepoTarget> {
    let cwd = std::env::current_dir()?;
    if let Some(ws_root) = find_workspace_root_opt(&cwd)? {
        if let Ok(ws) = WorkspaceConfig::load(&ws_root.join(WORKSPACE_FILE)) {
            if let Some(entry) = ws.find_repo(name) {
                let path = ws_root.join(&entry.repo.path);
                if is_git_repo(&path) {
                    return Ok(RepoTarget {
                        name: name.to_string(),
                        path,
                    });
                }
            }
        }
    }

    let source = repo_rs::SourceRoot::try_default()
        .map_err(|e| SunbeamError::Config(format!("repo-rs source root error: {e}")))?;
    let path = source
        .get_by_name(name)
        .map_err(|e| SunbeamError::Config(format!("repo-rs cannot resolve {name}: {e}")))?
        .ok_or_else(|| SunbeamError::Config(format!("repo-rs could not find repo {name}")))?;
    if !is_git_repo(&path) {
        return Err(SunbeamError::Config(format!(
            "resolved path {} is not a git repository",
            path.display()
        )));
    }
    Ok(RepoTarget {
        name: name.to_string(),
        path,
    })
}

fn is_git_repo(path: &Path) -> bool {
    if path.join(".git").exists() {
        return true;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Run a git subprocess in the given directory and stream output.
pub fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::ExitStatus> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(dir);
    cmd.args(args);
    let status = cmd.status().map_err(|e| SunbeamError::Io {
        context: format!("spawning git {:?} in {}", args, dir.display()),
        source: e,
    })?;
    if !status.success() {
        return Err(SunbeamError::ExternalTool {
            tool: "git".into(),
            detail: format!("git {:?} exited with {status}", args),
        });
    }
    Ok(status)
}

/// Run a git subprocess and capture stdout as lines.
pub fn run_git_output_lines(dir: &Path, args: &[&str]) -> Result<Vec<String>> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(dir);
    cmd.args(args);
    let out = cmd.output().map_err(|e| SunbeamError::Io {
        context: format!("spawning git {:?} in {}", args, dir.display()),
        source: e,
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SunbeamError::ExternalTool {
            tool: "git".into(),
            detail: format!("git {:?} failed: {}", args, stderr.trim()),
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().map(|s| s.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_git_repo_false_for_tmp() {
        let tmp = std::env::temp_dir();
        assert!(!is_git_repo(&tmp));
    }

    #[test]
    fn test_repo_target_debug() {
        let t = RepoTarget {
            name: "foo".into(),
            path: PathBuf::from("/tmp/foo"),
        };
        let s = format!("{t:?}");
        assert!(s.contains("foo"));
    }
}
