use crate::error::Result;
use crate::info;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_push(
    logger: &crate::logger::Logger,
    args: VcsArgs,
    remote: String,
    set_upstream: Option<String>,
) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        let mut git_args = vec!["push", &remote];
        if let Some(branch) = &set_upstream {
            git_args.push("-u");
            git_args.push(branch);
        }
        run_git(&target.path, &git_args)?;
        info!(logger, "Pushed", repo = target.name, remote = remote);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_cmd_push() {
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
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_push(&logger, args, "origin".into(), None)).unwrap();
    }

    #[test]
    fn test_cmd_push_set_upstream() {
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
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_push(&logger, args, "origin".into(), Some("main".into()))).unwrap();
    }
}
