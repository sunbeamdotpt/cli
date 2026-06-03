use crate::error::Result;
use crate::info;
use crate::vcs::{VcsArgs, resolve_targets, run_git_output_lines};

pub async fn cmd_status(logger: &crate::logger::Logger, args: VcsArgs) -> Result<()> {
    let targets = resolve_targets(&args)?;
    let multi = targets.len() > 1;
    let mut any_output = false;

    for target in targets {
        let lines = run_git_output_lines(&target.path, &["status", "--porcelain", "-b"])?;
        if lines.is_empty() {
            continue;
        }
        for line in &lines {
            if multi {
                info!(logger, &format!("[{:>12}]  {}", target.name, line));
            } else {
                info!(logger, line);
            }
            any_output = true;
        }
    }

    if !any_output && multi {
        info!(logger, "All repos clean.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::test_helpers::*;
    use tempfile::TempDir;

    #[test]
    fn test_prefix_format() {
        let name = "proxy";
        let line = " M src/main.rs";
        let out = format!("[{:>12}]  {}", name, line);
        assert_eq!(out, "[       proxy]   M src/main.rs");
    }

    #[test]
    fn test_cmd_status_clean_repo() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_status(&logger, args)).unwrap();
    }

    #[test]
    fn test_cmd_status_dirty_repo() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path());
        std::fs::write(tmp.path().join("dirty.txt"), "hello").unwrap();
        let _env = TestEnv::new(tmp.path());
        let args = VcsArgs {
            repo: None,
            all: false,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_status(&logger, args)).unwrap();
    }

    #[test]
    fn test_cmd_status_multi_repo() {
        let root = TempDir::new().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        git_init(&a);
        git_init(&b);
        std::fs::write(a.join("f.txt"), "a").unwrap();
        std::fs::write(b.join("f.txt"), "b").unwrap();
        write_workspace(root.path(), &[("a", "a"), ("b", "b")]);

        let _env = TestEnv::new(root.path());
        let args = VcsArgs {
            repo: None,
            all: true,
        };
        let logger = crate::logger::Logger::new(crate::logger::NoopSink);
        futures::executor::block_on(cmd_status(&logger, args)).unwrap();
    }
}
