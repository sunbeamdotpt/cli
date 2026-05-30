use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_fetch(args: VcsArgs, remote: String) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        run_git(&target.path, &["fetch", &remote])?;
        tracing::info!("Fetched {} in {}", remote, target.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_cmd_fetch() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");

        let bare = TempDir::new().unwrap();
        run_git(bare.path(), &["init", "--bare", "origin.git"]).unwrap();

        run_git(tmp.path(), &["remote", "add", "origin", &format!("{}/origin.git", bare.path().display())]).unwrap();

        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        futures::executor::block_on(cmd_fetch(args, "origin".into())).unwrap();
    }
}
