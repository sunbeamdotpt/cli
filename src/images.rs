use crate::cli::BuildTarget;
use anyhow::Result;

pub async fn cmd_build(_what: &BuildTarget, _push: bool, _deploy: bool) -> Result<()> {
    todo!("cmd_build: BuildKit gRPC builds")
}

pub async fn cmd_mirror() -> Result<()> {
    todo!("cmd_mirror: containerd-client + reqwest mirror")
}
