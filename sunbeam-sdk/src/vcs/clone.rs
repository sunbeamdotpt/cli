use std::path::PathBuf;

use crate::error::{Result, SunbeamError};
use crate::vcs::run_git;

pub async fn cmd_clone(url: String, name: Option<String>) -> Result<()> {
    let repo_name = name
        .or_else(|| extract_name_from_url(&url))
        .ok_or_else(|| {
            SunbeamError::Config("could not infer repo name from URL; pass --name".into())
        })?;

    let source = repo_rs::SourceRoot::try_default()
        .map_err(|e| SunbeamError::Config(format!("repo-rs source root error: {e}")))?;
    let dest = source.join(&repo_name);

    let parent = dest.parent().ok_or_else(|| {
        SunbeamError::Config(format!("repo-rs returned root path for {repo_name}"))
    })?;
    let dir_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| SunbeamError::Config(format!("invalid destination path for {repo_name}")))?;

    run_git(parent, &["clone", &url, dir_name])?;
    println!("Cloned {} into {}", url, dest.display());
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
    }

    #[test]
    fn test_cmd_clone_success() {
        let root = TempDir::new().unwrap();
        let bare = root.path().join("origin.git");
        std::fs::create_dir(&bare).unwrap();
        run_git(&bare, &["init", "--bare"]).unwrap();

        let code_root = root.path().join("code");
        std::fs::create_dir(&code_root).unwrap();

        let _env = TestEnv::new(root.path());
        _env.set_repo_code_root(&code_root);

        futures::executor::block_on(cmd_clone(
            format!("file://{}", bare.display()),
            Some("origin".into()),
        ))
        .unwrap();

        assert!(code_root.join("origin").join(".git").exists());
    }

    #[test]
    fn test_cmd_clone_infer_name() {
        let root = TempDir::new().unwrap();
        let bare = root.path().join("myrepo.git");
        std::fs::create_dir(&bare).unwrap();
        run_git(&bare, &["init", "--bare"]).unwrap();

        let code_root = root.path().join("code");
        std::fs::create_dir(&code_root).unwrap();

        let _env = TestEnv::new(root.path());
        _env.set_repo_code_root(&code_root);

        futures::executor::block_on(cmd_clone(
            format!("file://{}", bare.display()),
            None,
        ))
        .unwrap();

        assert!(code_root.join("myrepo").join(".git").exists());
    }

    #[test]
    fn test_cmd_clone_name_inference_fails() {
        let root = TempDir::new().unwrap();
        let _env = TestEnv::new(root.path());
        let err = futures::executor::block_on(cmd_clone("https://example.com/".into(), None)).unwrap_err();
        assert!(format!("{err}").contains("pass --name"));
    }
}
