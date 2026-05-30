use crate::error::Result;
use crate::vcs::{VcsArgs, resolve_targets, run_git};

pub async fn cmd_fetch(args: VcsArgs, remote: String) -> Result<()> {
    let targets = resolve_targets(&args)?;
    for target in targets {
        run_git(&target.path, &["fetch", &remote])?;
        println!("Fetched {} in {}", remote, target.name);
    }
    Ok(())
}
