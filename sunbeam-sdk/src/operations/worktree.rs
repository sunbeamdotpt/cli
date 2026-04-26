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

/// Current branch name on the main checkout (e.g. `mainline`).
fn current_branch_name(root: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .with_ctx(|| format!("git rev-parse --abbrev-ref HEAD in {}", root.display()))?;
    if !out.status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!("rev-parse --abbrev-ref HEAD failed (exit {})", out.status),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// If a worktree for `sanitized_branch` exists, return its on-disk path.
fn find_worktree_path(sanitized_branch: &str) -> Result<Option<PathBuf>> {
    let entries = list()?;
    Ok(entries
        .into_iter()
        .find(|e| e.branch == sanitized_branch)
        .map(|e| e.path))
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

    // Check whether the branch already exists locally. If so, we check it
    // out in the new worktree instead of creating it. A prior `sunbeam wt rm`
    // would have kept the branch unless --prune-branch was passed — that's
    // the normal path for "re-enter a feature branch".
    let branch_exists = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{sanitized}")])
        .current_dir(&root)
        .status()
        .ok()
        .map(|s| s.success())
        .unwrap_or(false);

    let target_str = target.to_string_lossy().into_owned();
    let mut args: Vec<String> = vec!["worktree".into(), "add".into(), target_str.clone()];
    if branch_exists {
        if from.is_some() {
            return Err(SunbeamError::config(format!(
                "branch {sanitized} already exists; --from <ref> only applies when creating a fresh branch"
            )));
        }
        args.push(sanitized.clone());
    } else {
        args.push("-b".into());
        args.push(sanitized.clone());
        if let Some(ref_) = from {
            args.push(ref_.to_string());
        }
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

    // Initialize submodules inside the new worktree. Best-effort: if a
    // submodule's pinned SHA is no longer reachable from its upstream
    // (force-pushed / rebased), git fetch will fail — but most submodules
    // still succeed. Continue with a warning rather than failing the whole
    // worktree creation. Users can re-run `git submodule update --init
    // --recursive` manually in the worktree, or `sunbeam wt setup`, once
    // upstream state settles.
    let sm_status = Command::new("git")
        .args(["submodule", "update", "--init", "--recursive"])
        .current_dir(&target)
        .status()
        .with_ctx(|| format!("spawning git submodule update in {target_str}"))?;
    if !sm_status.success() {
        tracing::warn!(
            "some submodules failed to init in {target_str} (exit {sm_status}); \
             worktree created. Re-run `sunbeam wt setup` or `git submodule update \
             --init --recursive` manually to retry individual submodules."
        );
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

/// Strategy for `merge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Rebase feature branch onto HEAD, then fast-forward HEAD. Linear history.
    Rebase,
    /// Classic `git merge` — creates a merge commit.
    MergeCommit,
    /// `git merge --squash` — one squashed commit on HEAD.
    Squash,
}

/// Merge `branch` into HEAD using the chosen strategy. Refuses to run from
/// inside any worktree — caller must be in the main checkout.
pub fn merge(branch: &str, strategy: MergeStrategy) -> Result<()> {
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

    if strategy == MergeStrategy::Rebase {
        // Rebase the feature branch onto the current HEAD of the main checkout,
        // then fast-forward the main branch to the rebased feature tip. This
        // keeps history strictly linear and re-writes the feature's commits
        // to sit directly on top of target.
        let target_branch = current_branch_name(&root)?;
        let wt_path = find_worktree_path(&sanitized)?;
        let rebase_dir = wt_path.unwrap_or_else(|| root.clone());
        let rebase_args: Vec<String> = if rebase_dir == root {
            // No worktree for this branch; rebase by specifying both refs.
            vec!["rebase".into(), target_branch.clone(), sanitized.clone()]
        } else {
            // Branch is checked out in a worktree; rebase there (no explicit
            // branch arg — git uses the worktree's current HEAD).
            vec!["rebase".into(), target_branch.clone()]
        };
        let rb_status = Command::new("git")
            .args(&rebase_args)
            .current_dir(&rebase_dir)
            .status()
            .with_ctx(|| format!("spawning git rebase in {}", rebase_dir.display()))?;
        if !rb_status.success() {
            return Err(SunbeamError::tool(
                "git",
                format!(
                    "rebase {sanitized} onto {target_branch} failed (exit {rb_status}). \
                     Resolve conflicts, then retry with `sunbeam wt merge {sanitized}` \
                     or finish in the worktree directly."
                ),
            ));
        }
        // Fast-forward main to the rebased tip.
        let ff_status = Command::new("git")
            .args(["merge", "--ff-only", &sanitized])
            .current_dir(&root)
            .status()
            .with_ctx(|| format!("spawning git merge --ff-only in {}", root.display()))?;
        if !ff_status.success() {
            return Err(SunbeamError::tool(
                "git",
                format!(
                    "fast-forward {target_branch} to {sanitized} failed (exit {ff_status})"
                ),
            ));
        }
        return Ok(());
    }

    let mut args: Vec<String> = vec!["merge".into()];
    if strategy == MergeStrategy::Squash {
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

/// Rebase `branch` onto `onto` (or the main checkout's current HEAD if
/// `onto` is `None`). Operates in the branch's worktree if one exists;
/// otherwise rebases by giving git both the target and branch refs.
pub fn rebase(branch: &str, onto: Option<&str>) -> Result<()> {
    let sanitized = sanitize_branch(branch);
    if sanitized.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {branch:?} is empty after sanitization"
        )));
    }
    let root = main_worktree_root()?;
    let target = match onto {
        Some(t) => t.to_string(),
        None => current_branch_name(&root)?,
    };
    let wt_path = find_worktree_path(&sanitized)?;
    let rebase_dir = wt_path.unwrap_or_else(|| root.clone());
    let args: Vec<String> = if rebase_dir == root {
        vec!["rebase".into(), target.clone(), sanitized.clone()]
    } else {
        vec!["rebase".into(), target.clone()]
    };
    let status = Command::new("git")
        .args(&args)
        .current_dir(&rebase_dir)
        .status()
        .with_ctx(|| format!("spawning git rebase in {}", rebase_dir.display()))?;
    if !status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!(
                "rebase {sanitized} onto {target} failed (exit {status}). Resolve \
                 conflicts in the worktree, then run `git rebase --continue`."
            ),
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

    // Pre-deinit submodules. `git worktree remove` refuses to remove worktrees
    // that contain initialized submodules (it doesn't want to orphan them).
    // Deinit first, then we can use `--force --force` safely — by this point
    // git's submodule-safety check is moot.
    if target.is_dir() {
        let _ = Command::new("git")
            .args(["submodule", "deinit", "--all", "--force"])
            .current_dir(&target)
            .status();
    }

    // Check for uncommitted work unless --force.
    if !force && target.is_dir() {
        let st = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&target)
            .output()
            .with_ctx(|| format!("git status in {}", target.display()))?;
        if st.status.success() && !st.stdout.is_empty() {
            return Err(SunbeamError::config(format!(
                "worktree has uncommitted work ({} entries); pass --force to discard",
                st.stdout.iter().filter(|&&b| b == b'\n').count()
            )));
        }
    }

    let target_str = target.to_string_lossy().into_owned();
    // Always pass `--force --force`. Single `--force` doesn't bypass git's
    // check for `.gitmodules` presence in the worktree, even post-deinit.
    // Second `--force` does. We've already gated dirty-file safety above.
    let args = ["worktree", "remove", "--force", "--force", &target_str];

    let status = Command::new("git")
        .args(args)
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

/// Resolve the absolute on-disk path of the worktree for `branch`.
///
/// Errors if the branch has no worktree. Used by shell wrappers
/// (`sunbeam wt shell-init <shell>`) to turn `wt use` into a real `cd`.
pub fn path_of(branch: &str) -> Result<PathBuf> {
    let sanitized = sanitize_branch(branch);
    if sanitized.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {branch:?} is empty after sanitization"
        )));
    }
    match find_worktree_path(&sanitized)? {
        Some(p) => Ok(p),
        None => Err(SunbeamError::config(format!(
            "no worktree for branch {sanitized} (try `sunbeam wt new {branch}`)"
        ))),
    }
}

/// Shell init scripts. Each defines a `sunbeam` function that intercepts
/// `wt use <branch>` (and `ops worktree use`/`operations worktree use`) and
/// performs a real `cd` in the caller's shell, exporting `SUNBEAM_WORKTREE`.
/// Everything else falls through to the binary.
pub fn shell_init_script(shell: crate::cli::WtShell) -> &'static str {
    match shell {
        crate::cli::WtShell::Bash | crate::cli::WtShell::Zsh => SHELL_INIT_BASH_ZSH,
        crate::cli::WtShell::Fish => SHELL_INIT_FISH,
    }
}

const SHELL_INIT_BASH_ZSH: &str = r#"# sunbeam worktree shell integration — source via:
#   eval "$(sunbeam wt shell-init zsh)"   # or bash
sunbeam() {
  local _is_use=0 _branch=""
  if [ "$1" = "wt" ] && [ "$2" = "use" ] && [ -n "${3-}" ]; then
    _is_use=1; _branch="$3"
  elif { [ "$1" = "ops" ] || [ "$1" = "operations" ]; } \
       && [ "$2" = "worktree" ] && [ "$3" = "use" ] && [ -n "${4-}" ]; then
    _is_use=1; _branch="$4"
  fi
  if [ $_is_use -eq 1 ]; then
    local _wt_path
    _wt_path="$(command sunbeam wt path "$_branch")" || return $?
    cd "$_wt_path" || return $?
    export SUNBEAM_WORKTREE="${_wt_path##*/}"
    return 0
  fi
  command sunbeam "$@"
}
"#;

const SHELL_INIT_FISH: &str = r#"# sunbeam worktree shell integration — source via:
#   sunbeam wt shell-init fish | source
function sunbeam
    set -l _is_use 0
    set -l _branch ""
    if test (count $argv) -ge 3; and test "$argv[1]" = "wt"; and test "$argv[2]" = "use"
        set _is_use 1
        set _branch $argv[3]
    else if test (count $argv) -ge 4; and begin; test "$argv[1]" = "ops"; or test "$argv[1]" = "operations"; end; and test "$argv[2]" = "worktree"; and test "$argv[3]" = "use"
        set _is_use 1
        set _branch $argv[4]
    end
    if test $_is_use -eq 1
        set -l _wt_path (command sunbeam wt path $_branch)
        or return $status
        cd $_wt_path
        or return $status
        set -gx SUNBEAM_WORKTREE (basename $_wt_path)
        return 0
    end
    command sunbeam $argv
end
"#;

/// Options for [`cherry_pick`].
#[derive(Debug, Clone)]
pub struct CherryPickOpts<'a> {
    pub from: &'a str,
    pub refs: &'a [String],
    pub into: Option<&'a str>,
    pub edit: bool,
    pub no_commit: bool,
    pub annotate: bool,
    pub mainline: Option<u32>,
    pub force: bool,
}

/// Cherry-pick commits from another worktree's branch into the destination.
///
/// Always passes `--signoff` to `git cherry-pick`. Worktrees share `.git`,
/// so refs from any branch are reachable without fetching. Refs starting
/// with `~` or `^`, and ranges with bare `~N`/`^N` segments, are expanded
/// against `opts.from`.
pub fn cherry_pick(opts: CherryPickOpts<'_>) -> Result<()> {
    let from_san = sanitize_branch(opts.from);
    if from_san.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {:?} is empty after sanitization",
            opts.from
        )));
    }
    let root = main_worktree_root()?;
    let from_exists = Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{from_san}"),
        ])
        .current_dir(&root)
        .status()
        .ok()
        .map(|s| s.success())
        .unwrap_or(false);
    if !from_exists {
        return Err(SunbeamError::config(format!(
            "no local branch {from_san} (try `sunbeam wt list`)"
        )));
    }

    let dest = resolve_cherry_pick_dest(opts.into, &root)?;

    if !opts.force {
        let st = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dest)
            .output()
            .with_ctx(|| format!("git status in {}", dest.display()))?;
        if st.status.success() && !st.stdout.is_empty() {
            return Err(SunbeamError::config(format!(
                "destination worktree {} is dirty; pass --force to override",
                dest.display()
            )));
        }
    }

    if opts.refs.is_empty() {
        return Err(SunbeamError::config("no commits/ranges supplied"));
    }
    let expanded: Vec<String> = opts
        .refs
        .iter()
        .map(|r| expand_ref_against(r, &from_san))
        .collect();

    let mut args: Vec<String> = vec!["cherry-pick".into(), "--signoff".into()];
    if opts.edit {
        args.push("--edit".into());
    }
    if opts.no_commit {
        args.push("--no-commit".into());
    }
    if opts.annotate {
        args.push("-x".into());
    }
    if let Some(m) = opts.mainline {
        args.push("--mainline".into());
        args.push(m.to_string());
    }
    args.extend(expanded);

    let status = Command::new("git")
        .args(&args)
        .current_dir(&dest)
        .status()
        .with_ctx(|| format!("git cherry-pick in {}", dest.display()))?;
    if !status.success() {
        return Err(SunbeamError::tool(
            "git",
            format!(
                "cherry-pick failed (exit {status}). Resolve conflicts in {}, \
                 then `git cherry-pick --continue` (or `--abort`).",
                dest.display()
            ),
        ));
    }
    Ok(())
}

fn resolve_cherry_pick_dest(into: Option<&str>, root: &Path) -> Result<PathBuf> {
    if let Some(b) = into {
        let san = sanitize_branch(b);
        if san.is_empty() {
            return Err(SunbeamError::config(format!(
                "branch name {b:?} is empty after sanitization"
            )));
        }
        return match find_worktree_path(&san)? {
            Some(p) => Ok(p),
            None => Err(SunbeamError::config(format!(
                "no worktree for branch {san} (try `sunbeam wt new {b}`)"
            ))),
        };
    }
    let cwd = std::env::current_dir()?;
    let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
    let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let worktrees_dir = root_canon.join(".worktrees");
    if cwd_canon.starts_with(&worktrees_dir) || cwd_canon == root_canon {
        Ok(cwd)
    } else {
        Err(SunbeamError::config(
            "cherry-pick must run from inside a worktree (or pass --into <branch>)",
        ))
    }
}

/// Expand a ref like `~3` / `~3..` / `~3..~1` into a fully-qualified form
/// against `from`. Bare SHAs and named refs (anything not starting with
/// `~` or `^` on the relevant side) pass through.
fn expand_ref_against(r: &str, from: &str) -> String {
    if let Some((l, rr)) = r.split_once("..") {
        let lhs = expand_part(l, from);
        let rhs = if rr.is_empty() {
            from.to_string()
        } else {
            expand_part(rr, from)
        };
        return format!("{lhs}..{rhs}");
    }
    expand_part(r, from)
}

fn expand_part(p: &str, from: &str) -> String {
    if p.is_empty() {
        return from.to_string();
    }
    if p.starts_with('~') || p.starts_with('^') {
        return format!("{from}{p}");
    }
    p.to_string()
}

/// Drop into an interactive `$SHELL` with cwd set to the worktree for `branch`.
/// Blocks until the user exits the shell.
pub fn use_shell(branch: &str) -> Result<()> {
    let sanitized = sanitize_branch(branch);
    if sanitized.is_empty() {
        return Err(SunbeamError::config(format!(
            "branch name {branch:?} is empty after sanitization"
        )));
    }
    let root = main_worktree_root()?;
    let dir = worktree_dir(&root, &sanitized);
    if !dir.is_dir() {
        return Err(SunbeamError::config(format!(
            "worktree path does not exist: {} (try `sunbeam wt new {branch}`)",
            dir.display()
        )));
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    eprintln!("☀ entering {} (exit to return)", dir.display());
    let status = Command::new(&shell)
        .current_dir(&dir)
        .env("SUNBEAM_WORKTREE", &sanitized)
        .status()
        .with_ctx(|| format!("spawning {shell}"))?;
    if !status.success() {
        // Non-zero exit from an interactive shell isn't an error for us —
        // the user may have `exit 1`'d deliberately. Just surface it.
        tracing::debug!("shell exited with {status}");
    }
    Ok(())
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

    #[test]
    fn expand_ref_passes_through_sha_and_named() {
        assert_eq!(expand_ref_against("abc1234", "feature-vpn"), "abc1234");
        assert_eq!(expand_ref_against("v2.0.0", "feature-vpn"), "v2.0.0");
    }

    #[test]
    fn expand_ref_prefixes_tilde_and_caret() {
        assert_eq!(expand_ref_against("~3", "feature-vpn"), "feature-vpn~3");
        assert_eq!(expand_ref_against("^2", "feature-vpn"), "feature-vpn^2");
    }

    #[test]
    fn expand_ref_range_open_right() {
        assert_eq!(
            expand_ref_against("~3..", "feature-vpn"),
            "feature-vpn~3..feature-vpn"
        );
    }

    #[test]
    fn expand_ref_range_both_relative() {
        assert_eq!(
            expand_ref_against("~5..~1", "feature-vpn"),
            "feature-vpn~5..feature-vpn~1"
        );
    }

    #[test]
    fn expand_ref_range_with_explicit_lhs() {
        assert_eq!(
            expand_ref_against("abc1234..~1", "feature-vpn"),
            "abc1234..feature-vpn~1"
        );
    }
}
