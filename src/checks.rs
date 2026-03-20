use anyhow::Result;

pub async fn cmd_check(_target: Option<&str>) -> Result<()> {
    todo!("cmd_check: concurrent health checks via reqwest + kube-rs")
}
