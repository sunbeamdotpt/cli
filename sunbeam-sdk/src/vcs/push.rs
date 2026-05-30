use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_push(args: VcsArgs, remote: String, set_upstream: Option<String>) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        let mut git_args = vec!["push", &remote];
        if let Some(branch) = &set_upstream {
            git_args.push("-u");
            git_args.push(branch);
        }
        run_git(&target.path, &git_args)?;
        println!("Pushed {} to {}", target.name, remote);
    }
    Ok(())
}
