use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_commit(args: VcsArgs, message: String, all: bool) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        let mut git_args = vec!["commit", "-m", &message];
        if all {
            git_args.push("-a");
        }
        run_git(&target.path, &git_args)?;
        println!("Committed in {}", target.name);
    }
    Ok(())
}
