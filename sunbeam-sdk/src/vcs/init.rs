//! Initialize workspace repos from a manifest.

use std::path::Path;

use crate::discovery::WORKSPACE_FILE;
use crate::error::{Result, SunbeamError};
use crate::operations::config::WorkspaceConfig;
use crate::vcs::run_git;

/// Initialize a workspace by cloning the root repo (if a URL is given) and then
/// cloning every sub-repo that has an `upstream` in the workspace manifest.
pub async fn cmd_init(url: Option<String>, name: Option<String>) -> Result<()> {
    let (ws_root, ws) = if let Some(url) = url {
        let repo_name = name
            .or_else(|| extract_name_from_url(&url))
            .ok_or_else(|| {
                SunbeamError::Config("could not infer repo name from URL; pass --name".into())
            })?;

        let cwd = std::env::current_dir()?;
        let dest = cwd.join(&repo_name);

        if dest.exists() {
            return Err(SunbeamError::Config(format!(
                "destination '{}' already exists",
                dest.display()
            )));
        }

        tracing::info!("Cloning workspace root into {repo_name}...");
        run_git(&cwd, &["clone", &url, &repo_name])?;

        let manifest_path = dest.join(WORKSPACE_FILE);
        if !manifest_path.exists() {
            tracing::info!("No {WORKSPACE_FILE} found in cloned repo; done.");
            return Ok(());
        }

        let ws = WorkspaceConfig::load(&manifest_path)?;
        (dest, ws)
    } else {
        let cwd = std::env::current_dir()?;
        let manifest_path = cwd.join(WORKSPACE_FILE);
        if !manifest_path.exists() {
            return Err(SunbeamError::Config(format!(
                "no {WORKSPACE_FILE} found in current directory and no URL given"
            )));
        }
        let ws = WorkspaceConfig::load(&manifest_path)?;
        (cwd, ws)
    };

    tracing::info!(
        "Workspace '{}' found. Initializing repos...",
        ws.workspace.name
    );

    let mut cloned = 0usize;
    let mut skipped = 0usize;

    for entry in ws.iter_repos() {
        let repo_path = ws_root.join(&entry.repo.path);

        if repo_path.exists() {
            tracing::info!("  [{}] already exists, skipping", entry.name);
            skipped += 1;
            continue;
        }

        let Some(upstream) = &entry.repo.upstream else {
            tracing::info!("  [{}] no upstream, skipping", entry.name);
            skipped += 1;
            continue;
        };

        let git_url = upstream_to_git_url(upstream);
        tracing::info!(
            "  [{}] cloning {} -> {}",
            entry.name, git_url, entry.repo.path
        );

        // Ensure parent directories exist so nested paths work.
        if let Some(parent) = repo_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        match run_git(&ws_root, &["clone", &git_url, &entry.repo.path]) {
            Ok(_) => cloned += 1,
            Err(e) => {
                tracing::warn!("  [{}] clone failed: {e}", entry.name);
                skipped += 1;
            }
        }
    }

    tracing::info!("Done: {cloned} cloned, {skipped} skipped.");
    Ok(())
}

fn extract_name_from_url(url: &str) -> Option<String> {
    let url = url.trim_end_matches(".git");
    let seg = url.rsplit('/').next()?;
    if seg.is_empty() {
        return None;
    }
    Some(seg.to_string())
}

fn upstream_to_git_url(upstream: &str) -> String {
    if upstream.starts_with("http://")
        || upstream.starts_with("https://")
        || upstream.starts_with("git@")
        || upstream.starts_with("file://")
    {
        upstream.to_string()
    } else if upstream.matches('/').count() >= 2 {
        format!("https://{}.git", upstream)
    } else {
        format!("https://github.com/{}.git", upstream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_extract_name_from_url() {
        assert_eq!(
            extract_name_from_url("https://github.com/foo/bar.git"),
            Some("bar".into())
        );
        assert_eq!(
            extract_name_from_url("git@github.com:foo/baz.git"),
            Some("baz".into())
        );
        assert_eq!(
            extract_name_from_url("https://example.com/qux"),
            Some("qux".into())
        );
        assert_eq!(extract_name_from_url("https://example.com/"), None);
    }

    #[test]
    fn test_upstream_to_git_url_github_slug() {
        assert_eq!(
            upstream_to_git_url("tokio-rs/tokio"),
            "https://github.com/tokio-rs/tokio.git"
        );
    }

    #[test]
    fn test_upstream_to_git_url_with_host() {
        assert_eq!(
            upstream_to_git_url("codeberg.org/forgejo/forgejo"),
            "https://codeberg.org/forgejo/forgejo.git"
        );
    }

    #[test]
    fn test_upstream_to_git_url_already_url() {
        assert_eq!(
            upstream_to_git_url("https://example.com/repo.git"),
            "https://example.com/repo.git"
        );
        assert_eq!(
            upstream_to_git_url("git@github.com:foo/bar.git"),
            "git@github.com:foo/bar.git"
        );
    }

    #[test]
    fn test_cmd_init_from_url() {
        let root = TempDir::new().unwrap();

        // Create a bare repo that will act as the workspace root.
        let bare = root.path().join("ws-root.git");
        std::fs::create_dir(&bare).unwrap();
        run_git(&bare, &["init", "--bare"]).unwrap();

        // Create bare repos for the sub-repos so cloning succeeds.
        let liba_bare = root.path().join("liba.git");
        std::fs::create_dir(&liba_bare).unwrap();
        run_git(&liba_bare, &["init", "--bare"]).unwrap();

        let svca_bare = root.path().join("svca.git");
        std::fs::create_dir(&svca_bare).unwrap();
        run_git(&svca_bare, &["init", "--bare"]).unwrap();

        // Seed the workspace bare repo with a manifest.
        let seed = root.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        git_init(&seed);
        let manifest = seed.join("sunbeam.workspace.yaml");
        std::fs::write(
            &manifest,
            format!(
                r#"
schema: 1
workspace:
  name: test-ws
  root: .
repos:
  3p:
    liba: {{ path: 3p/liba, upstream: file://{} }}
  forks:
    svca: {{ path: forks/svca, upstream: file://{} }}
  owned:
    myapp: {{ path: apps/myapp }}
"#,
                liba_bare.display(),
                svca_bare.display()
            ),
        )
        .unwrap();
        run_git(&seed, &["add", "."]).unwrap();
        run_git(&seed, &["commit", "-m", "init"]).unwrap();
        run_git(&seed, &["push", &format!("file://{}", bare.display()), "main"]).unwrap();
        // Ensure the bare repo's HEAD points to main so clones check out correctly.
        run_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]).unwrap();

        let _env = TestEnv::new(root.path());

        futures::executor::block_on(cmd_init(
            Some(format!("file://{}", bare.display())),
            Some("ws-root".into()),
        ))
        .unwrap();

        let ws_dir = root.path().join("ws-root");
        assert!(ws_dir.join("sunbeam.workspace.yaml").exists());
        assert!(ws_dir.join("3p/liba/.git").exists());
        assert!(ws_dir.join("forks/svca/.git").exists());
        assert!(!ws_dir.join("apps/myapp").exists());
    }

    #[test]
    fn test_cmd_init_local_manifest() {
        let root = TempDir::new().unwrap();

        let svc_bare = root.path().join("svc.git");
        std::fs::create_dir(&svc_bare).unwrap();
        run_git(&svc_bare, &["init", "--bare"]).unwrap();

        let manifest = root.path().join("sunbeam.workspace.yaml");
        std::fs::write(
            &manifest,
            format!(
                r#"
schema: 1
workspace:
  name: test-ws
  root: .
repos:
  forks:
    svc: {{ path: forks/svc, upstream: file://{} }}
"#,
                svc_bare.display()
            ),
        )
        .unwrap();

        let _env = TestEnv::new(root.path());

        futures::executor::block_on(cmd_init(None, None)).unwrap();

        assert!(root.path().join("forks/svc/.git").exists());
    }

    #[test]
    fn test_cmd_init_no_manifest_no_url() {
        let tmp = TempDir::new().unwrap();
        let _env = TestEnv::new(tmp.path());
        let err = futures::executor::block_on(cmd_init(None, None)).unwrap_err();
        assert!(format!("{err}").contains("no sunbeam.workspace.yaml"));
    }

    #[test]
    fn test_cmd_init_destination_exists() {
        let root = TempDir::new().unwrap();
        let bare = root.path().join("ws.git");
        std::fs::create_dir(&bare).unwrap();
        run_git(&bare, &["init", "--bare"]).unwrap();

        // Pre-create the destination directory.
        std::fs::create_dir(root.path().join("ws")).unwrap();

        let _env = TestEnv::new(root.path());
        let err = futures::executor::block_on(cmd_init(
            Some(format!("file://{}", bare.display())),
            None,
        ))
        .unwrap_err();
        assert!(format!("{err}").contains("already exists"));
    }

    #[test]
    fn test_cmd_init_clone_fail_continues() {
        let root = TempDir::new().unwrap();

        let manifest = root.path().join("sunbeam.workspace.yaml");
        std::fs::write(
            &manifest,
            r#"
schema: 1
workspace:
  name: test-ws
  root: .
repos:
  forks:
    badsvc: { path: forks/badsvc, upstream: example.com/org/badsvc }
"#,
        )
        .unwrap();

        // Do NOT create a bare repo for badsvc — clone will fail.
        let _env = TestEnv::new(root.path());

        // Should succeed overall even though the sub-repo clone fails.
        futures::executor::block_on(cmd_init(None, None)).unwrap();

        assert!(!root.path().join("forks/badsvc").exists());
    }
}
