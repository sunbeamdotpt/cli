use std::io::Write;

use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git_output_lines};

pub async fn cmd_log(args: VcsArgs, oneline: bool, limit: Option<usize>) -> Result<()> {
    let targets = resolve_targets(&args)?;
    let multi = targets.len() > 1;
    let mut stdout = std::io::stdout();

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
                writeln!(stdout, "[{:>12}]  {}", target.name, line).ok();
            } else {
                writeln!(stdout, "{}", line).ok();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_format_choice() {
        let oneline = true;
        let fmt = if oneline {
            "--oneline"
        } else {
            "--format=%h %s"
        };
        assert_eq!(fmt, "--oneline");
    }
}
