use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git_output_lines};

pub async fn cmd_log(args: VcsArgs, oneline: bool, limit: Option<usize>) -> Result<()> {
    let targets = resolve_targets(&args)?;
    let multi = targets.len() > 1;

    for target in targets {
        let mut git_args: Vec<String> = vec!["log".into()];
        if oneline {
            git_args.push("--oneline".into());
        } else {
            git_args.push("--format=%h %s (%cn, %ar)".into());
        }
        if let Some(n) = limit {
            git_args.push("-n".into());
            git_args.push(n.to_string());
        }
        let refs: Vec<&str> = git_args.iter().map(|s| s.as_str()).collect();
        let lines = run_git_output_lines(&target.path, &refs)?;
        for line in &lines {
            if multi {
                tracing::info!("[{:>12}]  {}", target.name, line);
            } else {
                tracing::info!("{}", line);
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
    fn test_format_choice() {
        let oneline = true;
        let fmt = if oneline { "--oneline" } else { "--format=%h %s" };
        assert_eq!(fmt, "--oneline");
    }

    #[test]
    fn test_cmd_log_default() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        futures::executor::block_on(cmd_log(args, false, None)).unwrap();
    }

    #[test]
    fn test_cmd_log_oneline() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        futures::executor::block_on(cmd_log(args, true, None)).unwrap();
    }

    #[test]
    fn test_cmd_log_limit() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        git_commit(tmp.path(), "first");
        git_commit(tmp.path(), "second");
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        futures::executor::block_on(cmd_log(args, true, Some(1))).unwrap();
    }

    #[test]
    fn test_cmd_log_multi_repo() {
        let root = TempDir::new().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        git_init(&a);
        git_init(&b);
        git_commit(&a, "a1");
        git_commit(&b, "b1");
        write_workspace(root.path(), &[("a", "a"), ("b", "b")]);

        let _env = TestEnv::new(root.path());
        let args = VcsArgs {
            repo: None,
            all: true,
        };
        futures::executor::block_on(cmd_log(args, true, None)).unwrap();
    }
}
