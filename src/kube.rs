use anyhow::{bail, Result};
use std::sync::OnceLock;

static CONTEXT: OnceLock<String> = OnceLock::new();
static SSH_HOST: OnceLock<String> = OnceLock::new();

/// Set the active kubectl context and optional SSH host for production tunnel.
pub fn set_context(ctx: &str, ssh_host: &str) {
    let _ = CONTEXT.set(ctx.to_string());
    let _ = SSH_HOST.set(ssh_host.to_string());
}

/// Get the active context.
pub fn context() -> &'static str {
    CONTEXT.get().map(|s| s.as_str()).unwrap_or("sunbeam")
}

/// Get the SSH host (empty for local).
pub fn ssh_host() -> &'static str {
    SSH_HOST.get().map(|s| s.as_str()).unwrap_or("")
}

/// Parse 'ns/name' -> (Some(ns), Some(name)), 'ns' -> (Some(ns), None), None -> (None, None).
pub fn parse_target(s: Option<&str>) -> Result<(Option<&str>, Option<&str>)> {
    match s {
        None => Ok((None, None)),
        Some(s) => {
            let parts: Vec<&str> = s.splitn(3, '/').collect();
            match parts.len() {
                1 => Ok((Some(parts[0]), None)),
                2 => Ok((Some(parts[0]), Some(parts[1]))),
                _ => bail!("Invalid target {s:?}: expected 'namespace' or 'namespace/name'"),
            }
        }
    }
}

/// Replace all occurrences of DOMAIN_SUFFIX with domain.
pub fn domain_replace(text: &str, domain: &str) -> String {
    text.replace("DOMAIN_SUFFIX", domain)
}

/// Transparent kubectl passthrough for the active context.
pub async fn cmd_k8s(_kubectl_args: &[String]) -> Result<()> {
    todo!("cmd_k8s: kubectl passthrough via kube-rs")
}

/// Run bao CLI inside the OpenBao pod with the root token.
pub async fn cmd_bao(_bao_args: &[String]) -> Result<()> {
    todo!("cmd_bao: bao passthrough via kube-rs exec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_target_none() {
        let (ns, name) = parse_target(None).unwrap();
        assert!(ns.is_none());
        assert!(name.is_none());
    }

    #[test]
    fn test_parse_target_namespace_only() {
        let (ns, name) = parse_target(Some("ory")).unwrap();
        assert_eq!(ns, Some("ory"));
        assert!(name.is_none());
    }

    #[test]
    fn test_parse_target_namespace_and_name() {
        let (ns, name) = parse_target(Some("ory/kratos")).unwrap();
        assert_eq!(ns, Some("ory"));
        assert_eq!(name, Some("kratos"));
    }

    #[test]
    fn test_parse_target_too_many_parts() {
        assert!(parse_target(Some("too/many/parts")).is_err());
    }

    #[test]
    fn test_parse_target_empty_string() {
        let (ns, name) = parse_target(Some("")).unwrap();
        assert_eq!(ns, Some(""));
        assert!(name.is_none());
    }

    #[test]
    fn test_domain_replace_single() {
        let result = domain_replace("src.DOMAIN_SUFFIX/foo", "192.168.1.1.sslip.io");
        assert_eq!(result, "src.192.168.1.1.sslip.io/foo");
    }

    #[test]
    fn test_domain_replace_multiple() {
        let result = domain_replace("DOMAIN_SUFFIX and DOMAIN_SUFFIX", "x.sslip.io");
        assert_eq!(result, "x.sslip.io and x.sslip.io");
    }

    #[test]
    fn test_domain_replace_none() {
        let result = domain_replace("no match here", "x.sslip.io");
        assert_eq!(result, "no match here");
    }
}
