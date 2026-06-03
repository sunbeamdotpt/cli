use crate::error::Result;
use crate::info;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_fetch(logger: &crate::logger::Logger, args: VcsArgs, remote: String) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        run_git(&target.path, &["fetch", &remote])?;
        info!(logger, "Fetched", remote = remote, repo = target.name);
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
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_fetch(&logger, args, "origin".into())).unwrap();
    }
}
