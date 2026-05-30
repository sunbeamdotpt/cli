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
pub mod test_helpers {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    pub struct TestEnv {
        pub old_cwd: PathBuf,
        pub old_repo_code_root: Option<String>,
        pub old_sunbeam_workspace: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TestEnv {
        pub fn new(cwd: &Path) -> Self {
            let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let old_cwd = std::env::current_dir().unwrap();
            let old_repo_code_root = std::env::var("REPO_CODE_ROOT").ok();
            let old_sunbeam_workspace = std::env::var("SUNBEAM_WORKSPACE").ok();
            unsafe { std::env::remove_var("SUNBEAM_WORKSPACE"); }
            std::env::set_current_dir(cwd).unwrap();
            TestEnv {
                old_cwd,
                old_repo_code_root,
                old_sunbeam_workspace,
                _guard: guard,
            }
        }

        pub fn set_repo_code_root(&self, path: &Path) {
            unsafe { std::env::set_var("REPO_CODE_ROOT", path); }
        }

        pub fn set_sunbeam_workspace(&self, path: &Path) {
            unsafe { std::env::set_var("SUNBEAM_WORKSPACE", path); }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                match &self.old_repo_code_root {
                    Some(v) => std::env::set_var("REPO_CODE_ROOT", v),
                    None => std::env::remove_var("REPO_CODE_ROOT"),
                }
                match &self.old_sunbeam_workspace {
                    Some(v) => std::env::set_var("SUNBEAM_WORKSPACE", v),
                    None => std::env::remove_var("SUNBEAM_WORKSPACE"),
                }
            }
            std::env::set_current_dir(&self.old_cwd).unwrap();
        }
    }

    pub fn git_init(path: &Path) {
        let status = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .current_dir(path)
            .status()
            .expect("git init failed");
        assert!(status.success());
        Command::new("git")
            .args(["config", "user.email", "test@sunbeam.pt"])
            .current_dir(path)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .status()
            .unwrap();
    }

    pub fn git_commit(path: &Path, msg: &str) {
        let file = path.join("file.txt");
        std::fs::write(&file, msg).unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", msg, "--quiet"])
            .current_dir(path)
            .status()
            .unwrap();
    }

    pub fn write_workspace(path: &Path, repos: &[(&str, &str)]) {
        let mut yaml = String::from("schema: 1\nworkspace:\n  name: test\n  root: .\nrepos:\n  owned:\n");
        for (name, p) in repos {
            yaml.push_str(&format!("    {name}: {{ path: {p} }}\n"));
        }
        std::fs::write(path.join("sunbeam.workspace.yaml"), yaml).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_is_git_repo_false_for_tmp() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_git_repo(tmp.path()));
    }

    #[test]
    fn test_is_git_repo_true() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        assert!(is_git_repo(tmp.path()));
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

    #[test]
    fn test_run_git_success() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        let status = run_git(tmp.path(), &["rev-parse", "--git-dir"]).unwrap();
        assert!(status.success());
    }

    #[test]
    fn test_run_git_failure() {
        let tmp = TempDir::new().unwrap();
        let err = run_git(tmp.path(), &["not-a-command"]).unwrap_err();
        assert!(format!("{err}").contains("git"));
    }

    #[test]
    fn test_run_git_output_lines_success() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        let lines = run_git_output_lines(tmp.path(), &["rev-parse", "--git-dir"]).unwrap();
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_run_git_output_lines_failure() {
        let tmp = TempDir::new().unwrap();
        let err = run_git_output_lines(tmp.path(), &["not-a-command"]).unwrap_err();
        assert!(format!("{err}").contains("git"));
    }

    #[test]
    fn test_resolve_targets_cwd() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let targets = resolve_targets(&args).unwrap();
        assert_eq!(targets.len(), 1);
        assert!(targets[0].path.ends_with(tmp.path().file_name().unwrap()));
    }

    #[test]
    fn test_resolve_targets_no_repo_no_cwd() {
        let tmp = TempDir::new().unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let err = resolve_targets(&args).unwrap_err();
        assert!(format!("{err}").contains("not inside a git repo"));
    }

    #[test]
    fn test_resolve_targets_all_workspace() {
        let root = TempDir::new().unwrap();
        let repo_a = root.path().join("a");
        let repo_b = root.path().join("b");
        std::fs::create_dir(&repo_a).unwrap();
        std::fs::create_dir(&repo_b).unwrap();
        git_init(&repo_a);
        git_init(&repo_b);
        write_workspace(root.path(), &[("a", "a"), ("b", "b")]);

        let _env = TestEnv::new(root.path());
        let args = VcsArgs {
            repo: None,
            all: true,
        };
        let targets = resolve_targets(&args).unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn test_resolve_targets_all_no_workspace() {
        let tmp = TempDir::new().unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: true,
        };
        let err = resolve_targets(&args).unwrap_err();
        assert!(format!("{err}").contains("--all requires a sunbeam workspace"));
    }

    #[test]
    fn test_resolve_targets_repo_workspace_match() {
        let root = TempDir::new().unwrap();
        let repo_a = root.path().join("a");
        std::fs::create_dir(&repo_a).unwrap();
        git_init(&repo_a);
        write_workspace(root.path(), &[("a", "a")]);

        let _env = TestEnv::new(root.path());
        let args = VcsArgs {
            repo: Some("a".into()),
            all: false,
        };
        let targets = resolve_targets(&args).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "a");
    }

    #[test]
    fn test_resolve_targets_repo_rs_fallback() {
        let root = TempDir::new().unwrap();
        let code_root = root.path().join("code");
        std::fs::create_dir(&code_root).unwrap();
        let repo_path = code_root.join("myrepo");
        std::fs::create_dir(&repo_path).unwrap();
        git_init(&repo_path);

        let _env = TestEnv::new(root.path());
        _env.set_repo_code_root(&code_root);

        let args = VcsArgs {
            repo: Some("myrepo".into()),
            all: false,
        };
        let targets = resolve_targets(&args).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "myrepo");
    }

    #[test]
    fn test_resolve_targets_repo_not_found() {
        let tmp = TempDir::new().unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: Some("nonexistent".into()),
            all: false,
        };
        let err = resolve_targets(&args).unwrap_err();
        assert!(format!("{err}").contains("nonexistent"));
    }

    #[test]
    fn test_resolve_workspace_all_skips_non_git() {
        let root = TempDir::new().unwrap();
        let repo_a = root.path().join("a");
        let not_git = root.path().join("not-git");
        std::fs::create_dir(&repo_a).unwrap();
        std::fs::create_dir(&not_git).unwrap();
        git_init(&repo_a);
        write_workspace(root.path(), &[("a", "a"), ("b", "not-git")]);

        let _env = TestEnv::new(root.path());
        let args = VcsArgs {
            repo: None,
            all: true,
        };
        let targets = resolve_targets(&args).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "a");
    }
}
