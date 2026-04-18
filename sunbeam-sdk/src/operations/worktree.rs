//! Per-branch git worktree lifecycle management.
//!
//! Manages git worktrees under `<repo_root>/.worktrees/<sanitized-branch>/`
//! with optional post-create setup via `.hooks/setup`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, ResultExt, SunbeamError};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub branch: String,
    pub path: PathBuf,
    pub head: String,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_locked: bool,
    pub dirty: u32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sanitize a branch name into a filesystem-safe directory name.
///
/// Collapses any run of non-`[A-Za-z0-9_-]` characters into a single `-`,
/// then trims leading/trailing dashes.
pub fn sanitize_branch(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
        if ok {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Discover the top-level git repository root of the current cwd.
pub fn repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    repo_root_at(&cwd)
}

fn repo_root_at(dir: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .with_ctx(|| format!("spawning git rev-parse in {}", dir.display()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SunbeamError::tool(
            "git",
            format!("rev-parse --show-toplevel failed: {}", stderr.trim()),
        ));
    }

    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return Err(SunbeamError::tool(
            "git",
            "rev-parse --show-toplevel returned empty",
        ));
    }
    Ok(PathBuf::from(path))
}

/// Return the common dir (the main working tree root) even from inside a worktree.
///
/// Used to ensure `.worktrees/` always lives beside the main checkout.
fn main_worktree_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let out = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(&cwd)
        .output()
        .with_ctx(|| format!("spawning git rev-parse --git-common-dir in {}", cwd.display()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SunbeamError::tool(
            "git",
            format!("rev-parse --git-common-dir failed: {}", stderr.trim()),
        ));
    }

    let common_dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    // common_dir is typically `<main>/.git`; the parent is the main worktree root.
    let root = common_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| SunbeamError::tool("git", "common dir has no parent"))?;
    Ok(root)
}

fn worktree_dir(root: &Path, sanitized: &str) -> PathBuf {
    root.join(".worktrees").join(sanitized)
}

fn current_dir_inside(path: &Path) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    let Ok(cwd_canon) = std::fs::canonicalize(&cwd) else {
        return false;
    };
    let Ok(path_canon) = std::fs::canonicalize(path) else {
        return false;
    };
    cwd_canon.starts_with(&path_canon)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Create a new worktree for `branch` rooted at `.worktrees/<sanitized>`.
///
/// Initializes submodules and optionally runs `.hooks/setup` inside the new tree.
pub fn new(branch: &str, from: Option<&str>, run_setup: bool) -> Result<PathBuf> {
    let sanitized = sanitize_branch(branch);
    if sanitized.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {branch:?} is empty after sanitization"
        )));
    }

    let root = main_worktree_root()?;
    let target = worktree_dir(&root, &sanitized);

    if target.exists() {
        return Err(SunbeamError::config(format!(
            "worktree path already exists: {}",
            target.display()
        )));
    }

    let target_str = target.to_string_lossy().into_owned();
    let mut args: Vec<String> = vec![
        "worktree".into(),
        "add".into(),
        target_str.clone(),
        "-b".into(),
        sanitized.clone(),
    ];
    if let Some(ref_) = from {
        args.push(ref_.to_string());
    }

    let status = Command::new("git")
        .args(&args)
        .current_dir(&root)
        .status()
        .with_ctx(|| format!("spawning git worktree add in {}", root.display()))?;
    if !status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!("worktree add failed (exit {status})"),
        ));
    }

    // Initialize submodules inside the new worktree.
    let sm_status = Command::new("git")
        .args(["submodule", "update", "--init", "--recursive"])
        .current_dir(&target)
        .status()
        .with_ctx(|| format!("spawning git submodule update in {target_str}"))?;
    if !sm_status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!("submodule update --init --recursive failed (exit {sm_status})"),
        ));
    }

    if run_setup {
        run_setup_hook(&target)?;
    }

    Ok(target)
}

/// Enumerate existing worktrees by parsing `git worktree list --porcelain`.
pub fn list() -> Result<Vec<WorktreeEntry>> {
    let root = main_worktree_root()?;
    let out = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&root)
        .output()
        .with_ctx(|| format!("spawning git worktree list in {}", root.display()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(SunbeamError::tool(
            "git",
            format!("worktree list --porcelain failed: {}", stderr.trim()),
        ));
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries = parse_porcelain(&text);

    // Enrich each entry with the dirty-file count from `git status --porcelain`.
    for e in &mut entries {
        if !e.path.is_dir() {
            continue;
        }
        let dirty = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&e.path)
            .output();
        if let Ok(st) = dirty {
            if st.status.success() {
                let text = String::from_utf8_lossy(&st.stdout);
                e.dirty = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count() as u32;
            }
        }
    }

    Ok(entries)
}

/// Merge `branch` into HEAD. Refuses to run from inside any worktree —
/// the caller must be in the main checkout to keep merges predictable.
pub fn merge(branch: &str, squash: bool) -> Result<()> {
    let sanitized = sanitize_branch(branch);
    if sanitized.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {branch:?} is empty after sanitization"
        )));
    }

    let root = main_worktree_root()?;
    let cwd = std::env::current_dir()?;
    let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let root_canon = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let worktrees_dir = root_canon.join(".worktrees");
    if cwd_canon.starts_with(&worktrees_dir) {
        return Err(SunbeamError::config(
            "refuse to merge from inside a worktree — run this from the main checkout",
        ));
    }

    let mut args: Vec<String> = vec!["merge".into()];
    if squash {
        args.push("--squash".into());
    }
    args.push(sanitized);

    let status = Command::new("git")
        .args(&args)
        .current_dir(&root)
        .status()
        .with_ctx(|| format!("spawning git merge in {}", root.display()))?;
    if !status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!("merge failed (exit {status})"),
        ));
    }
    Ok(())
}

/// Remove the worktree for `branch` (and optionally delete the branch).
pub fn remove(branch: &str, force: bool, prune_branch: bool) -> Result<()> {
    let sanitized = sanitize_branch(branch);
    if sanitized.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {branch:?} is empty after sanitization"
        )));
    }

    let root = main_worktree_root()?;
    let target = worktree_dir(&root, &sanitized);

    if current_dir_inside(&target) {
        return Err(SunbeamError::config(format!(
            "refuse to remove worktree while cwd is inside it: {}",
            target.display()
        )));
    }

    let target_str = target.to_string_lossy().into_owned();
    let mut args: Vec<String> = vec!["worktree".into(), "remove".into()];
    if force {
        args.push("--force".into());
    }
    args.push(target_str);

    let status = Command::new("git")
        .args(&args)
        .current_dir(&root)
        .status()
        .with_ctx(|| format!("spawning git worktree remove in {}", root.display()))?;
    if !status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!("worktree remove failed (exit {status})"),
        ));
    }

    if prune_branch {
        let del_flag = if force { "-D" } else { "-d" };
        let br_status = Command::new("git")
            .args(["branch", del_flag, &sanitized])
            .current_dir(&root)
            .status()
            .with_ctx(|| format!("spawning git branch {del_flag} {sanitized}"))?;
        if !br_status.success() {
            return Err(SunbeamError::tool(
                "git",
                format!("branch {del_flag} {sanitized} failed (exit {br_status})"),
            ));
        }
    }

    Ok(())
}

/// Re-run `.hooks/setup` in the named worktree, or in the current working dir
/// if no name is given.
pub fn setup(branch: Option<&str>) -> Result<()> {
    let target = match branch {
        None => std::env::current_dir()?,
        Some(b) => {
            let sanitized = sanitize_branch(b);
            if sanitized.is_empty() {
                return Err(SunbeamError::config(format!(
                    "branch name {b:?} is empty after sanitization"
                )));
            }
            let root = main_worktree_root()?;
            let dir = worktree_dir(&root, &sanitized);
            if !dir.is_dir() {
                return Err(SunbeamError::config(format!(
                    "worktree path does not exist: {}",
                    dir.display()
                )));
            }
            dir
        }
    };
    run_setup_hook(&target)
}

fn run_setup_hook(dir: &Path) -> Result<()> {
    let hook = dir.join(".hooks").join("setup");
    if !hook.is_file() {
        // No hook — silently skip.
        return Ok(());
    }
    let hook_str = hook.to_string_lossy().into_owned();
    let status = Command::new("bash")
        .arg(&hook_str)
        .current_dir(dir)
        .status()
        .with_ctx(|| format!("spawning bash {hook_str}"))?;
    if !status.success() {
        return Err(SunbeamError::tool(
            ".hooks/setup",
            format!("exited with status {status}"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Porcelain parser
// ---------------------------------------------------------------------------

/// Parse the output of `git worktree list --porcelain`.
///
/// Format (per entry, terminated by a blank line):
///
/// ```text
/// worktree /abs/path
/// HEAD <sha>
/// branch refs/heads/<name>      (or "detached")
/// bare                          (optional)
/// locked <reason>               (optional)
/// ```
fn parse_porcelain(text: &str) -> Vec<WorktreeEntry> {
    let mut out: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for line in text.lines() {
        if line.trim().is_empty() {
            if let Some(e) = current.take() {
                out.push(e);
            }
            continue;
        }

        let (key, rest) = match line.split_once(' ') {
            Some((k, r)) => (k, r),
            None => (line, ""),
        };

        match key {
            "worktree" => {
                if let Some(e) = current.take() {
                    out.push(e);
                }
                current = Some(WorktreeEntry {
                    branch: String::new(),
                    path: PathBuf::from(rest),
                    head: String::new(),
                    is_bare: false,
                    is_detached: false,
                    is_locked: false,
                    dirty: 0,
                });
            }
            "HEAD" => {
                if let Some(e) = current.as_mut() {
                    e.head = rest.to_string();
                }
            }
            "branch" => {
                if let Some(e) = current.as_mut() {
                    e.branch = rest
                        .strip_prefix("refs/heads/")
                        .unwrap_or(rest)
                        .to_string();
                }
            }
            "detached" => {
                if let Some(e) = current.as_mut() {
                    e.is_detached = true;
                }
            }
            "bare" => {
                if let Some(e) = current.as_mut() {
                    e.is_bare = true;
                }
            }
            "locked" => {
                if let Some(e) = current.as_mut() {
                    e.is_locked = true;
                }
            }
            _ => {}
        }
    }

    if let Some(e) = current.take() {
        out.push(e);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_slash() {
        assert_eq!(sanitize_branch("feature/vpn"), "feature-vpn");
    }

    #[test]
    fn sanitize_collapses_and_trims() {
        assert_eq!(sanitize_branch("WIP — notes!"), "WIP-notes");
        assert_eq!(sanitize_branch("---foo---"), "foo");
        assert_eq!(sanitize_branch("a//b///c"), "a-b-c");
    }

    #[test]
    fn sanitize_preserves_underscore_and_digits() {
        assert_eq!(sanitize_branch("v1_2_release"), "v1_2_release");
        assert_eq!(sanitize_branch("issue-42"), "issue-42");
    }

    #[test]
    fn sanitize_all_bad_is_empty() {
        assert_eq!(sanitize_branch("!!!"), "");
        assert_eq!(sanitize_branch("   "), "");
    }

    #[test]
    fn parse_porcelain_main_plus_branch() {
        let raw = "\
worktree /home/u/repo
HEAD abc123def456
branch refs/heads/main

worktree /home/u/repo/.worktrees/feature-vpn
HEAD deadbeef0000
branch refs/heads/feature-vpn

worktree /home/u/repo/.worktrees/detached
HEAD 1111222233334444
detached
";
        let entries = parse_porcelain(raw);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].head, "abc123def456");
        assert_eq!(entries[0].path, PathBuf::from("/home/u/repo"));
        assert!(!entries[0].is_detached);

        assert_eq!(entries[1].branch, "feature-vpn");
        assert_eq!(
            entries[1].path,
            PathBuf::from("/home/u/repo/.worktrees/feature-vpn")
        );

        assert!(entries[2].is_detached);
        assert_eq!(entries[2].branch, "");
    }

    #[test]
    fn parse_porcelain_bare_and_locked() {
        let raw = "\
worktree /srv/mirror.git
HEAD 0000000000000000
bare

worktree /srv/work/stable
HEAD 1234567890abcdef
branch refs/heads/stable
locked manual hold
";
        let entries = parse_porcelain(raw);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_bare);
        assert!(entries[1].is_locked);
        assert_eq!(entries[1].branch, "stable");
    }

    #[test]
    fn parse_porcelain_empty() {
        assert!(parse_porcelain("").is_empty());
        assert!(parse_porcelain("\n\n").is_empty());
    }
}
