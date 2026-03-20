use anyhow::Result;

pub async fn cmd_user_list(_search: &str) -> Result<()> {
    todo!("cmd_user_list: ory-kratos-client SDK")
}

pub async fn cmd_user_get(_target: &str) -> Result<()> {
    todo!("cmd_user_get: ory-kratos-client SDK")
}

pub async fn cmd_user_create(_email: &str, _name: &str, _schema_id: &str) -> Result<()> {
    todo!("cmd_user_create: ory-kratos-client SDK")
}

pub async fn cmd_user_delete(_target: &str) -> Result<()> {
    todo!("cmd_user_delete: ory-kratos-client SDK")
}

pub async fn cmd_user_recover(_target: &str) -> Result<()> {
    todo!("cmd_user_recover: ory-kratos-client SDK")
}

pub async fn cmd_user_disable(_target: &str) -> Result<()> {
    todo!("cmd_user_disable: ory-kratos-client SDK")
}

pub async fn cmd_user_enable(_target: &str) -> Result<()> {
    todo!("cmd_user_enable: ory-kratos-client SDK")
}

pub async fn cmd_user_set_password(_target: &str, _password: &str) -> Result<()> {
    todo!("cmd_user_set_password: ory-kratos-client SDK")
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_user_onboard(
    _email: &str,
    _name: &str,
    _schema_id: &str,
    _send_email: bool,
    _notify: &str,
    _job_title: &str,
    _department: &str,
    _office_location: &str,
    _hire_date: &str,
    _manager: &str,
) -> Result<()> {
    todo!("cmd_user_onboard: ory-kratos-client SDK + lettre SMTP")
}

pub async fn cmd_user_offboard(_target: &str) -> Result<()> {
    todo!("cmd_user_offboard: ory-kratos-client + ory-hydra-client SDK")
}
