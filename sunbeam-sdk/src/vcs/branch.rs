use std::io::Write;

use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git, run_git_output_lines};

pub async fn cmd_branch(
    args: VcsArgs,
    list: bool,
    create: Option<String>,
    delete: Option<String>,
) -> Result<()> {
    let targets = resolve_targets(&args)?;
    let multi = targets.len() > 1;
    let mut stdout = std::io::stdout();

    if let Some(ref name) = create {
        for target in targets {
            run_git(&target.path, &["branch", name])?;
            println!("Created branch '{}' in {}", name, target.name);
        }
        return Ok(());
    }
    if let Some(ref name) = delete {
        for target in targets {
            run_git(&target.path, &["branch", "-D", name])?;
            println!("Deleted branch '{}' in {}", name, target.name);
        }
        return Ok(());
    }

    for target in targets {
        let lines = run_git_output_lines(&target.path, &["branch", "--format=%(refname:short)"])?;
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
