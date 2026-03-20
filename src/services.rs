use anyhow::Result;

pub async fn cmd_status(_target: Option<&str>) -> Result<()> {
    todo!("cmd_status: pod health via kube-rs")
}

pub async fn cmd_logs(_target: &str, _follow: bool) -> Result<()> {
    todo!("cmd_logs: stream pod logs via kube-rs")
}

pub async fn cmd_get(_target: &str, _output: &str) -> Result<()> {
    todo!("cmd_get: get pod via kube-rs")
}

pub async fn cmd_restart(_target: Option<&str>) -> Result<()> {
    todo!("cmd_restart: rollout restart via kube-rs")
}
