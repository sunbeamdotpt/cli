//! Walk ancestors to locate a project's `sunbeam.yaml` or a workspace's
//! `sunbeam.workspace.yaml`.
//!
//! A "project" is a directory containing `sunbeam.yaml`. A "workspace" is
//! a directory containing `sunbeam.workspace.yaml`. Both are found by
//! walking parent directories until one is located or the filesystem root
//! is reached.
//!
//! For workspaces, `$SUNBEAM_WORKSPACE` takes precedence over ancestor
//! traversal so CI and scripts can pin a specific workspace.

use std::env;
use std::path::{Path, PathBuf};

use crate::error::{Result, SunbeamError};

/// Filename used to mark a Sunbeam project directory.
pub const PROJECT_FILE: &str = "sunbeam.yaml";

/// Filename used to mark a Sunbeam workspace root.
pub const WORKSPACE_FILE: &str = "sunbeam.workspace.yaml";

/// Environment variable that, if set, overrides workspace discovery.
pub const WORKSPACE_ENV: &str = "SUNBEAM_WORKSPACE";

/// Walk from `start` up to the filesystem root looking for `sunbeam.yaml`.
///
/// Returns the directory containing the file (not the file itself).
pub fn find_project_root(start: &Path) -> Result<PathBuf> {
    find_ancestor(start, PROJECT_FILE).ok_or_else(|| {
        SunbeamError::Config(format!(
            "no {PROJECT_FILE} found in {} or any parent directory",
            start.display()
        ))
    })
}

/// Find the workspace root.
///
/// Resolution order:
/// 1. `$SUNBEAM_WORKSPACE` env var, if set and non-empty.
/// 2. Walk ancestors of `start` looking for `sunbeam.workspace.yaml`.
///
/// The env var must point to a directory that actually contains
/// `sunbeam.workspace.yaml` — we don't trust it blindly.
pub fn find_workspace_root(start: &Path) -> Result<PathBuf> {
    if let Some(dir) = env_workspace()? {
        return Ok(dir);
    }
    find_ancestor(start, WORKSPACE_FILE).ok_or_else(|| {
        SunbeamError::Config(format!(
            "no {WORKSPACE_FILE} found in {} or any parent directory",
            start.display()
        ))
    })
}

/// Like `find_workspace_root`, but returns `Ok(None)` instead of erroring
/// when no workspace is found. Useful for commands that want to opportunistically
/// pick up workspace context without requiring it.
pub fn find_workspace_root_opt(start: &Path) -> Result<Option<PathBuf>> {
    // Treat an invalid SUNBEAM_WORKSPACE as "no workspace" and fall back to
    // ancestor traversal rather than surfacing an error.
    if let Some(dir) = env_workspace().ok().flatten() {
        return Ok(Some(dir));
    }
    Ok(find_ancestor(start, WORKSPACE_FILE))
}

fn env_workspace() -> Result<Option<PathBuf>> {
    let Ok(raw) = env::var(WORKSPACE_ENV) else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let dir = PathBuf::from(&raw);
    if !dir.join(WORKSPACE_FILE).is_file() {
        return Err(SunbeamError::Config(format!(
            "{WORKSPACE_ENV}={raw} does not contain {WORKSPACE_FILE}"
        )));
    }
    Ok(Some(dir))
}

fn find_ancestor(start: &Path, filename: &str) -> Option<PathBuf> {
    let mut current: &Path = start;
    loop {
        if current.join(filename).is_file() {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Env-var tests mutate process-global state. Serialize them so concurrent
    // test threads don't race on SUNBEAM_WORKSPACE.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn touch(path: &Path) {
        fs::write(path, "schema: 1\n").unwrap();
    }

    #[test]
    fn finds_project_in_current_dir() {
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join(PROJECT_FILE));
        let found = find_project_root(tmp.path()).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[test]
    fn finds_project_in_ancestor() {
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join(PROJECT_FILE));
        let nested = tmp.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        let found = find_project_root(&nested).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[test]
    fn project_missing_errors() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("deep");
        fs::create_dir_all(&nested).unwrap();
        let err = find_project_root(&nested).unwrap_err();
        assert!(err.to_string().contains(PROJECT_FILE));
    }

    #[test]
    fn finds_workspace_via_ancestor() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Isolate from any ambient SUNBEAM_WORKSPACE in the test env.
        let _guard = EnvGuard::unset(WORKSPACE_ENV);
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join(WORKSPACE_FILE));
        let nested = tmp.path().join("repo").join("src");
        fs::create_dir_all(&nested).unwrap();
        let found = find_workspace_root(&nested).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[test]
    fn workspace_env_override_wins() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        touch(&tmp.path().join(WORKSPACE_FILE));
        let _guard = EnvGuard::set(WORKSPACE_ENV, tmp.path().to_str().unwrap());

        // Call from an unrelated dir — env should still resolve.
        let elsewhere = TempDir::new().unwrap();
        let found = find_workspace_root(elsewhere.path()).unwrap();
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[test]
    fn workspace_env_without_file_errors() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _guard = EnvGuard::set(WORKSPACE_ENV, tmp.path().to_str().unwrap());
        let err = find_workspace_root(tmp.path()).unwrap_err();
        assert!(err.to_string().contains(WORKSPACE_FILE));
    }

    #[test]
    fn workspace_opt_returns_none_when_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::unset(WORKSPACE_ENV);
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("no_ws");
        fs::create_dir_all(&nested).unwrap();
        assert!(find_workspace_root_opt(&nested).unwrap().is_none());
    }

    // Env tests mutate process-global state. These tests aren't parallel-safe
    // against each other if nextest runs them in the same process, but our
    // workspace configures nextest with per-test processes via fork.
    struct EnvGuard {
        key: String,
        prior: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let prior = env::var(key).ok();
            unsafe { env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prior,
            }
        }

        fn unset(key: &str) -> Self {
            let prior = env::var(key).ok();
            unsafe { env::remove_var(key) };
            Self {
                key: key.to_string(),
                prior,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(v) => unsafe { env::set_var(&self.key, v) },
                None => unsafe { env::remove_var(&self.key) },
            }
        }
    }
}
