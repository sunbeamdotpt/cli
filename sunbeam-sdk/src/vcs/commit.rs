use crate::error::Result;
use crate::info;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_commit(logger: &crate::logger::Logger, args: VcsArgs, message: String, all: bool) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        let mut git_args = vec!["commit", "-m", &message];
        if all {
            git_args.push("-a");
        }
        run_git(&target.path, &git_args)?;
        info!(logger, "Committed", repo = target.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use crate::vcs::run_git_output_lines;
    use tempfile::TempDir;

    #[test]
    fn test_cmd_commit() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), "hello").unwrap();
        run_git(tmp.path(), &["add", "."]).unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_commit(&logger, args, "test commit".into(), false)).unwrap();
        let log = run_git_output_lines(tmp.path(), &["log", "--oneline"]).unwrap();
        assert!(!log.is_empty());
        assert!(log[0].contains("test commit"));
    }

    #[test]
    fn test_cmd_commit_all() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        std::fs::write(tmp.path().join("file.txt"), "modified").unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_commit(&logger, args, "auto commit".into(), true)).unwrap();
        let log = run_git_output_lines(tmp.path(), &["log", "--oneline"]).unwrap();
        assert!(log[0].contains("auto commit"));
    }
}
