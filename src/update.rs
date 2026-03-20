use anyhow::Result;

/// Compile-time commit SHA set by build.rs.
pub const COMMIT: &str = env!("SUNBEAM_COMMIT");

pub async fn cmd_update() -> Result<()> {
    todo!("cmd_update: self-update from latest mainline commit via Gitea API")
}

pub fn cmd_version() {
    println!("sunbeam {COMMIT}");
}
