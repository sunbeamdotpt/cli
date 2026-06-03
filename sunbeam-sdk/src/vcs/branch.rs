use crate::error::Result;
use crate::info;
use crate::vcs::{VcsArgs, resolve_targets, run_git, run_git_output_lines};

pub async fn cmd_branch(
    logger: &crate::logger::Logger,
    args: VcsArgs,
    _list: bool,
    create: Option<String>,
    delete: Option<String>,
) -> Result<()> {
    let targets = resolve_targets(&args)?;
    let multi = targets.len() > 1;

    if let Some(ref name) = create {
        for target in targets {
            run_git(&target.path, &["branch", name])?;
            info!(logger, "Created branch", branch = name, repo = target.name);
        }
        return Ok(());
    }
    if let Some(ref name) = delete {
        for target in targets {
            run_git(&target.path, &["branch", "-D", name])?;
            info!(logger, "Deleted branch", branch = name, repo = target.name);
        }
        return Ok(());
    }

    for target in targets {
        let lines =
            run_git_output_lines(&target.path, &["branch", "--format=%(refname:short)"])?;
        for line in &lines {
            if multi {
                info!(logger, &format!("[{:>12}]  {}", target.name, line));
            } else {
                info!(logger, line);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_cmd_branch_list() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_branch(&logger, args, true, None, None)).unwrap();
    }

    #[test]
    fn test_cmd_branch_create() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_branch(&logger, args, false, Some("feature".into()), None)).unwrap();
        let branches = run_git_output_lines(tmp.path(), &["branch", "--format=%(refname:short)"]).unwrap();
        assert!(branches.iter().any(|b| b == "feature"));
    }

    #[test]
    fn test_cmd_branch_delete() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        run_git(tmp.path(), &["branch", "tmp-branch"]).unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_branch(&logger, args, false, None, Some("tmp-branch".into()))).unwrap();
        let branches = run_git_output_lines(tmp.path(), &["branch", "--format=%(refname:short)"]).unwrap();
        assert!(!branches.iter().any(|b| b == "tmp-branch"));
    }
}
