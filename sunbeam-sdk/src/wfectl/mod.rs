//! Remote workflow management via wfe-server gRPC API.
//!
//! This module embeds the `wfectl` command set into the sunbeam CLI so that
//! users can run `sunbeam workflows <subcommand>` instead of a separate binary.
//! Auth is handled by sunbeam's SSO flow (`sunbeam auth sso`).

pub mod cancel;
/// Client.
pub mod client;
/// Definitions.
pub mod definitions;
/// Get.
pub mod get;
/// List.
pub mod list;
/// Logs.
pub mod logs;
/// Output.
pub mod output;
/// Publish.
pub mod publish;
/// Register.
pub mod register;
/// Resume.
pub mod resume;
/// Run.
pub mod run;
/// Search logs.
pub mod search_logs;
/// Struct util.
pub mod struct_util;
/// Suspend.
pub mod suspend;
/// Validate.
pub mod validate;
/// Watch.
pub mod watch;

/// Resolve the SSO access token from the unified config auth store.
pub fn resolve_token(domain: &str) -> anyhow::Result<String> {
    let tokens = crate::config::get_auth_tokens(domain)
        .ok_or_else(|| anyhow::anyhow!("not logged in — run `sunbeam auth login` first"))?;
    if tokens.access_token.is_empty() {
        return Err(anyhow::anyhow!(
            "token cache is corrupt — run `sunbeam auth login` again"
        ));
    }
    Ok(tokens.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_token_missing_returns_not_logged_in() {
        let err = resolve_token("nonexistent.example.com").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not logged in"), "unexpected: {msg}");
    }
}
