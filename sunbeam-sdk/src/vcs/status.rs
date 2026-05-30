use std::io::Write;

use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git_output_lines};

pub async fn cmd_status(args: VcsArgs) -> Result<()> {
    let targets = resolve_targets(&args)?;
    let multi = targets.len() > 1;
    let mut stdout = std::io::stdout();
    let mut any_output = false;

    for target in targets {
        let lines = run_git_output_lines(&target.path, &["status", "--porcelain", "-b"])?;
        if lines.is_empty() {
            continue;
        }
        for line in &lines {
            if multi {
                writeln!(stdout, "[{:>12}]  {}", target.name, line).ok();
            } else {
                writeln!(stdout, "{}", line).ok();
            }
            any_output = true;
        }
    }

    if !any_output && multi {
        println!("All repos clean.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_prefix_format() {
        let name = "proxy";
        let line = " M src/main.rs";
        let out = format!("[{:>12}]  {}", name, line);
        assert_eq!(out, "[       proxy]   M src/main.rs");
    }
}
