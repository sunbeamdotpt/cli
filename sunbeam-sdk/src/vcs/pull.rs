use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_pull(args: VcsArgs, remote: String) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        run_git(&target.path, &["pull", &remote])?;
        tracing::info!("Pulled {} in {}", remote, target.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_cmd_pull() {
        let bare = TempDir::new().unwrap();
        run_git(bare.path(), &["init", "--bare", "origin.git"]).unwrap();

        // Seed the bare repo with a commit via a temp repo
        let seed = TempDir::new().unwrap();
        git_init(seed.path());
        git_commit(seed.path(), "first");
        run_git(seed.path(), &["remote", "add", "origin", &format!("{}/origin.git", bare.path().display())]).unwrap();
        run_git(seed.path(), &["push", "-u", "origin", "main"]).unwrap();

        // Clone the bare repo and pull
        let clone = TempDir::new().unwrap();
        run_git(clone.path(), &["clone", "--branch", "main", &format!("{}/origin.git", bare.path().display()), "repo"]).unwrap();
        let repo_path = clone.path().join("repo");

        let _env = TestEnv::new(&repo_path);
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        futures::executor::block_on(cmd_pull(args, "origin".into())).unwrap();
    }
}
