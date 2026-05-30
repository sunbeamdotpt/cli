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
    let dest = source
        .get_by_name(&repo_name)
        .map_err(|e| {
            SunbeamError::Config(format!(
                "repo-rs cannot resolve destination for {repo_name}: {e}"
            ))
        })?
        .ok_or_else(|| {
            SunbeamError::Config(format!(
                "repo-rs could not find destination for {repo_name}"
            ))
        })?;

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
    url.rsplit('/').next().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
