use crate::error::{Result, SunbeamError};
use clap::{Parser, Subcommand, ValueEnum};

/// Sunbeam local dev stack manager.
#[derive(Parser, Debug)]
#[command(name = "sunbeam", about = "Sunbeam local dev stack manager")]
pub struct Cli {
    /// Named context to use (overrides current-context from config).
    #[arg(long)]
    pub context: Option<String>,

    /// Domain suffix override (e.g. sunbeam.pt).
    #[arg(long, default_value = "")]
    pub domain: String,

    /// ACME email for cert-manager (e.g. ops@sunbeam.pt).
    #[arg(long, default_value = "")]
    pub email: String,

    #[command(subcommand)]
    pub verb: Option<Verb>,
}


#[derive(Subcommand, Debug)]
pub enum Verb {
    /// Full cluster bring-up.
    Up,

    /// Pod health (optionally scoped).
    Status {
        /// namespace or namespace/name
        target: Option<String>,
    },

    /// kustomize build + domain subst + kubectl apply.
    Apply {
        /// Limit apply to one namespace.
        namespace: Option<String>,
        /// Apply all namespaces without confirmation.
        #[arg(long = "all")]
        apply_all: bool,
        /// Domain suffix (e.g. sunbeam.pt).
        #[arg(long, default_value = "")]
        domain: String,
        /// ACME email for cert-manager.
        #[arg(long, default_value = "")]
        email: String,
    },

    /// Generate/store all credentials in OpenBao.
    Seed,

    /// E2E VSO + OpenBao integration test.
    Verify,

    /// kubectl logs for a service.
    Logs {
        /// namespace/name
        target: String,
        /// Stream logs.
        #[arg(short, long)]
        follow: bool,
    },

    /// Raw kubectl get for a pod (ns/name).
    Get {
        /// namespace/name
        target: String,
        /// Output format.
        #[arg(short, long, default_value = "yaml", value_parser = ["yaml", "json", "wide"])]
        output: String,
    },

    /// Rolling restart of services.
    Restart {
        /// namespace or namespace/name
        target: Option<String>,
    },

    /// Build an artifact.
    Build {
        /// What to build.
        what: BuildTarget,
        /// Push image to registry after building.
        #[arg(long)]
        push: bool,
        /// Apply manifests and rollout restart after pushing (implies --push).
        #[arg(long)]
        deploy: bool,
        /// Disable buildkitd layer cache.
        #[arg(long)]
        no_cache: bool,
    },

    /// Functional service health checks.
    Check {
        /// namespace or namespace/name
        target: Option<String>,
    },

    /// Mirror amd64-only La Suite images.
    Mirror,

    /// Create Gitea orgs/repos; bootstrap services.
    Bootstrap,

    /// Manage sunbeam configuration.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// kubectl passthrough.
    K8s {
        /// arguments forwarded verbatim to kubectl
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        kubectl_args: Vec<String>,
    },

    /// bao CLI passthrough (runs inside OpenBao pod with root token).
    Bao {
        /// arguments forwarded verbatim to bao
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        bao_args: Vec<String>,
    },

    /// User/identity management.
    User {
        #[command(subcommand)]
        action: Option<UserAction>,
    },

    /// Authenticate with Sunbeam (OAuth2 login via browser).
    Auth {
        #[command(subcommand)]
        action: Option<AuthAction>,
    },

    /// Project management across Planka and Gitea.
    Pm {
        #[command(subcommand)]
        action: Option<PmAction>,
    },

    /// Workflow management (list, status, retry, cancel, run).
    Workflow {
        #[command(subcommand)]
        action: crate::workflows::cmd::WorkflowAction,
    },

    /// Deploy service(s) — apply manifests + rollout restart.
    Deploy {
        /// Service name, category, or namespace (e.g. "hydra", "auth", "ory").
        /// Use --all for everything.
        target: Option<String>,
        /// Deploy all services.
        #[arg(long)]
        all: bool,
    },

    /// View secrets for a service.
    Secrets {
        /// Service name (e.g. "hydra").
        service: String,
        #[command(subcommand)]
        action: Option<SecretsAction>,
    },

    /// Interactive shell into a service.
    Shell {
        /// Service name (e.g. "postgres", "gitea").
        service: String,
    },

    /// Connect to the cluster VPN (foreground; Ctrl-C to disconnect).
    Connect,

    /// Disconnect from the cluster VPN.
    Disconnect,

    /// VPN diagnostics and key management.
    Vpn {
        #[command(subcommand)]
        action: VpnAction,
    },

    /// Self-update from latest mainline commit.
    Update,

    /// Print version info.
    Version,
}

#[derive(Subcommand, Debug)]
pub enum VpnAction {
    /// Show VPN tunnel status.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum SecretsAction {
    /// Get a specific secret field value.
    Get {
        /// Field name within the service's KV path.
        key: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Log in to both SSO and Gitea.
    Login {
        /// Domain to authenticate against (e.g. sunbeam.pt).
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to SSO only (Hydra OIDC — for Planka, identity management).
    Sso {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log in to Gitea only (personal access token).
    Git {
        /// Domain to authenticate against.
        #[arg(long)]
        domain: Option<String>,
    },
    /// Log out (remove all cached tokens).
    Logout,
    /// Show current authentication status.
    Status,
    /// Print the current access token (for use in scripts and MCP headers).
    Token,
}

#[derive(Subcommand, Debug)]
pub enum PmAction {
    /// List tickets across Planka and Gitea.
    List {
        /// Filter by source: planka, gitea, or all (default: all).
        #[arg(long, default_value = "all")]
        source: String,
        /// Filter by state: open, closed, all (default: open).
        #[arg(long, default_value = "open")]
        state: String,
    },
    /// Show ticket details.
    Show {
        /// Ticket ID (e.g. p:42 for Planka, g:studio/cli#7 for Gitea).
        id: String,
    },
    /// Create a new ticket.
    Create {
        /// Ticket title.
        title: String,
        /// Ticket body/description.
        #[arg(long, default_value = "")]
        body: String,
        /// Source: planka or gitea.
        #[arg(long, default_value = "gitea")]
        source: String,
        /// Target: board ID for Planka, or org/repo for Gitea.
        #[arg(long, default_value = "")]
        target: String,
    },
    /// Add a comment to a ticket.
    Comment {
        /// Ticket ID.
        id: String,
        /// Comment text.
        text: String,
    },
    /// Close/complete a ticket.
    Close {
        /// Ticket ID.
        id: String,
    },
    /// Assign a user to a ticket.
    Assign {
        /// Ticket ID.
        id: String,
        /// Username or email to assign.
        user: String,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum BuildTarget {
    Proxy,
    Integration,
    KratosAdmin,
    Meet,
    DocsFrontend,
    PeopleFrontend,
    People,
    Messages,
    MessagesBackend,
    MessagesFrontend,
    MessagesMtaIn,
    MessagesMtaOut,
    MessagesMpa,
    MessagesSocksProxy,
    Tuwunel,
    Calendars,
    Projects,
    Sol,
}

impl std::fmt::Display for BuildTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BuildTarget::Proxy => "proxy",
            BuildTarget::Integration => "integration",
            BuildTarget::KratosAdmin => "kratos-admin",
            BuildTarget::Meet => "meet",
            BuildTarget::DocsFrontend => "docs-frontend",
            BuildTarget::PeopleFrontend => "people-frontend",
            BuildTarget::People => "people",
            BuildTarget::Messages => "messages",
            BuildTarget::MessagesBackend => "messages-backend",
            BuildTarget::MessagesFrontend => "messages-frontend",
            BuildTarget::MessagesMtaIn => "messages-mta-in",
            BuildTarget::MessagesMtaOut => "messages-mta-out",
            BuildTarget::MessagesMpa => "messages-mpa",
            BuildTarget::MessagesSocksProxy => "messages-socks-proxy",
            BuildTarget::Tuwunel => "tuwunel",
            BuildTarget::Calendars => "calendars",
            BuildTarget::Projects => "projects",
            BuildTarget::Sol => "sol",
        };
        write!(f, "{s}")
    }
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Set configuration values for the current context.
    Set {
        /// Domain suffix (e.g. sunbeam.pt).
        #[arg(long, default_value = "")]
        domain: String,
        /// Production SSH host (e.g. user@server.example.com).
        #[arg(long, default_value = "")]
        host: String,
        /// Infrastructure directory root.
        #[arg(long, default_value = "")]
        infra_dir: String,
        /// ACME email for Let's Encrypt certificates.
        #[arg(long, default_value = "")]
        acme_email: String,
        /// Context name to configure (default: current context).
        #[arg(long, default_value = "")]
        context_name: String,
    },
    /// Get current configuration.
    Get,
    /// Clear configuration.
    Clear,
    /// Switch the active context.
    UseContext {
        /// Context name to switch to.
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserAction {
    /// List identities.
    List {
        /// Filter by email.
        #[arg(long, default_value = "")]
        search: String,
    },
    /// Get identity by email or ID.
    Get {
        /// Email or identity ID.
        target: String,
    },
    /// Create identity.
    Create {
        /// Email address.
        email: String,
        /// Display name.
        #[arg(long, default_value = "")]
        name: String,
        /// Schema ID.
        #[arg(long, default_value = "default")]
        schema: String,
    },
    /// Delete identity.
    Delete {
        /// Email or identity ID.
        target: String,
    },
    /// Generate recovery link.
    Recover {
        /// Email or identity ID.
        target: String,
    },
    /// Disable identity + revoke sessions (lockout).
    Disable {
        /// Email or identity ID.
        target: String,
    },
    /// Re-enable a disabled identity.
    Enable {
        /// Email or identity ID.
        target: String,
    },
    /// Set password for an identity.
    SetPassword {
        /// Email or identity ID.
        target: String,
        /// New password. If omitted, reads from stdin.
        password: Option<String>,
    },
    /// Onboard new user (create + welcome email).
    Onboard {
        /// Email address.
        email: String,
        /// Display name (First Last).
        #[arg(long, default_value = "")]
        name: String,
        /// Schema ID.
        #[arg(long, default_value = "employee")]
        schema: String,
        /// Skip sending welcome email.
        #[arg(long)]
        no_email: bool,
        /// Send welcome email to this address instead.
        #[arg(long, default_value = "")]
        notify: String,
        /// Job title.
        #[arg(long, default_value = "")]
        job_title: String,
        /// Department.
        #[arg(long, default_value = "")]
        department: String,
        /// Office location.
        #[arg(long, default_value = "")]
        office_location: String,
        /// Hire date (YYYY-MM-DD).
        #[arg(long, default_value = "", value_parser = validate_date)]
        hire_date: String,
        /// Manager name or email.
        #[arg(long, default_value = "")]
        manager: String,
    },
    /// Offboard user (disable + revoke all).
    Offboard {
        /// Email or identity ID.
        target: String,
    },
}

fn validate_date(s: &str) -> std::result::Result<String, String> {
    if s.is_empty() {
        return Ok(s.to_string());
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|_| s.to_string())
        .map_err(|_| format!("Invalid date: '{s}' (expected YYYY-MM-DD)"))
}


#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    // 1. test_up
    #[test]
    fn test_up() {
        let cli = parse(&["sunbeam", "up"]);
        assert!(matches!(cli.verb, Some(Verb::Up)));
    }

    // 2. test_status_no_target
    #[test]
    fn test_status_no_target() {
        let cli = parse(&["sunbeam", "status"]);
        match cli.verb {
            Some(Verb::Status { target }) => assert!(target.is_none()),
            _ => panic!("expected Status"),
        }
    }

    // 3. test_status_with_namespace
    #[test]
    fn test_status_with_namespace() {
        let cli = parse(&["sunbeam", "status", "ory"]);
        match cli.verb {
            Some(Verb::Status { target }) => assert_eq!(target.unwrap(), "ory"),
            _ => panic!("expected Status"),
        }
    }

    // 4. test_logs_no_follow
    #[test]
    fn test_logs_no_follow() {
        let cli = parse(&["sunbeam", "logs", "ory/kratos"]);
        match cli.verb {
            Some(Verb::Logs { target, follow }) => {
                assert_eq!(target, "ory/kratos");
                assert!(!follow);
            }
            _ => panic!("expected Logs"),
        }
    }

    // 5. test_logs_follow_short
    #[test]
    fn test_logs_follow_short() {
        let cli = parse(&["sunbeam", "logs", "ory/kratos", "-f"]);
        match cli.verb {
            Some(Verb::Logs { follow, .. }) => assert!(follow),
            _ => panic!("expected Logs"),
        }
    }

    // 6. test_build_proxy
    #[test]
    fn test_build_proxy() {
        let cli = parse(&["sunbeam", "build", "proxy"]);
        match cli.verb {
            Some(Verb::Build { what, push, deploy, no_cache }) => {
                assert!(matches!(what, BuildTarget::Proxy));
                assert!(!push);
                assert!(!deploy);
                assert!(!no_cache);
            }
            _ => panic!("expected Build"),
        }
    }

    // 7. test_build_deploy_flag
    #[test]
    fn test_build_deploy_flag() {
        let cli = parse(&["sunbeam", "build", "proxy", "--deploy"]);
        match cli.verb {
            Some(Verb::Build { deploy, push, no_cache, .. }) => {
                assert!(deploy);
                // clap does not imply --push; that logic is in dispatch()
                assert!(!push);
                assert!(!no_cache);
            }
            _ => panic!("expected Build"),
        }
    }

    // 8. test_build_invalid_target
    #[test]
    fn test_build_invalid_target() {
        let result = Cli::try_parse_from(&["sunbeam", "build", "notavalidtarget"]);
        assert!(result.is_err());
    }

    // 9. test_user_set_password
    #[test]
    fn test_user_set_password() {
        let cli = parse(&["sunbeam", "user", "set-password", "admin@example.com", "hunter2"]);
        match cli.verb {
            Some(Verb::User { action: Some(UserAction::SetPassword { target, password }) }) => {
                assert_eq!(target, "admin@example.com");
                assert_eq!(password, Some("hunter2".to_string()));
            }
            _ => panic!("expected User SetPassword"),
        }
    }

    #[test]
    fn test_user_set_password_no_password() {
        let cli = parse(&["sunbeam", "user", "set-password", "admin@example.com"]);
        match cli.verb {
            Some(Verb::User { action: Some(UserAction::SetPassword { target, password }) }) => {
                assert_eq!(target, "admin@example.com");
                assert!(password.is_none());
            }
            _ => panic!("expected User SetPassword"),
        }
    }

    // 10. test_user_onboard_basic
    #[test]
    fn test_user_onboard_basic() {
        let cli = parse(&["sunbeam", "user", "onboard", "a@b.com"]);
        match cli.verb {
            Some(Verb::User { action: Some(UserAction::Onboard {
                email, name, schema, no_email, notify, ..
            }) }) => {
                assert_eq!(email, "a@b.com");
                assert_eq!(name, "");
                assert_eq!(schema, "employee");
                assert!(!no_email);
                assert_eq!(notify, "");
            }
            _ => panic!("expected User Onboard"),
        }
    }

    // 11. test_user_onboard_full
    #[test]
    fn test_user_onboard_full() {
        let cli = parse(&[
            "sunbeam", "user", "onboard", "a@b.com",
            "--name", "A B", "--schema", "default", "--no-email",
            "--job-title", "Engineer", "--department", "Dev",
            "--office-location", "Paris", "--hire-date", "2026-01-15",
            "--manager", "boss@b.com",
        ]);
        match cli.verb {
            Some(Verb::User { action: Some(UserAction::Onboard {
                email, name, schema, no_email, job_title,
                department, office_location, hire_date, manager, ..
            }) }) => {
                assert_eq!(email, "a@b.com");
                assert_eq!(name, "A B");
                assert_eq!(schema, "default");
                assert!(no_email);
                assert_eq!(job_title, "Engineer");
                assert_eq!(department, "Dev");
                assert_eq!(office_location, "Paris");
                assert_eq!(hire_date, "2026-01-15");
                assert_eq!(manager, "boss@b.com");
            }
            _ => panic!("expected User Onboard"),
        }
    }

    // 12. test_apply_no_namespace
    #[test]
    fn test_apply_no_namespace() {
        let cli = parse(&["sunbeam", "apply"]);
        match cli.verb {
            Some(Verb::Apply { namespace, .. }) => assert!(namespace.is_none()),
            _ => panic!("expected Apply"),
        }
    }

    // 13. test_apply_with_namespace
    #[test]
    fn test_apply_with_namespace() {
        let cli = parse(&["sunbeam", "apply", "lasuite"]);
        match cli.verb {
            Some(Verb::Apply { namespace, .. }) => assert_eq!(namespace.unwrap(), "lasuite"),
            _ => panic!("expected Apply"),
        }
    }

    // 14. test_config_set
    #[test]
    fn test_config_set() {
        let cli = parse(&[
            "sunbeam", "config", "set",
            "--host", "user@example.com",
            "--infra-dir", "/path/to/infra",
        ]);
        match cli.verb {
            Some(Verb::Config { action: Some(ConfigAction::Set { host, infra_dir, .. }) }) => {
                assert_eq!(host, "user@example.com");
                assert_eq!(infra_dir, "/path/to/infra");
            }
            _ => panic!("expected Config Set"),
        }
    }

    // 15. test_config_get / test_config_clear
    #[test]
    fn test_config_get() {
        let cli = parse(&["sunbeam", "config", "get"]);
        match cli.verb {
            Some(Verb::Config { action: Some(ConfigAction::Get) }) => {}
            _ => panic!("expected Config Get"),
        }
    }

    #[test]
    fn test_config_clear() {
        let cli = parse(&["sunbeam", "config", "clear"]);
        match cli.verb {
            Some(Verb::Config { action: Some(ConfigAction::Clear) }) => {}
            _ => panic!("expected Config Clear"),
        }
    }

    // 16. test_no_args_prints_help
    #[test]
    fn test_no_args_prints_help() {
        let cli = parse(&["sunbeam"]);
        assert!(cli.verb.is_none());
    }

    // 17. test_get_json_output
    #[test]
    fn test_get_json_output() {
        let cli = parse(&["sunbeam", "get", "ory/kratos-abc", "-o", "json"]);
        match cli.verb {
            Some(Verb::Get { target, output }) => {
                assert_eq!(target, "ory/kratos-abc");
                assert_eq!(output, "json");
            }
            _ => panic!("expected Get"),
        }
    }

    // 18. test_check_with_target
    #[test]
    fn test_check_with_target() {
        let cli = parse(&["sunbeam", "check", "devtools"]);
        match cli.verb {
            Some(Verb::Check { target }) => assert_eq!(target.unwrap(), "devtools"),
            _ => panic!("expected Check"),
        }
    }

    // 19. test_build_messages_components
    #[test]
    fn test_build_messages_backend() {
        let cli = parse(&["sunbeam", "build", "messages-backend"]);
        match cli.verb {
            Some(Verb::Build { what, .. }) => {
                assert!(matches!(what, BuildTarget::MessagesBackend));
            }
            _ => panic!("expected Build"),
        }
    }

    #[test]
    fn test_build_messages_frontend() {
        let cli = parse(&["sunbeam", "build", "messages-frontend"]);
        match cli.verb {
            Some(Verb::Build { what, .. }) => {
                assert!(matches!(what, BuildTarget::MessagesFrontend));
            }
            _ => panic!("expected Build"),
        }
    }

    #[test]
    fn test_build_messages_mta_in() {
        let cli = parse(&["sunbeam", "build", "messages-mta-in"]);
        match cli.verb {
            Some(Verb::Build { what, .. }) => {
                assert!(matches!(what, BuildTarget::MessagesMtaIn));
            }
            _ => panic!("expected Build"),
        }
    }

    #[test]
    fn test_build_messages_mta_out() {
        let cli = parse(&["sunbeam", "build", "messages-mta-out"]);
        match cli.verb {
            Some(Verb::Build { what, .. }) => {
                assert!(matches!(what, BuildTarget::MessagesMtaOut));
            }
            _ => panic!("expected Build"),
        }
    }

    #[test]
    fn test_build_messages_mpa() {
        let cli = parse(&["sunbeam", "build", "messages-mpa"]);
        match cli.verb {
            Some(Verb::Build { what, .. }) => {
                assert!(matches!(what, BuildTarget::MessagesMpa));
            }
            _ => panic!("expected Build"),
        }
    }

    #[test]
    fn test_build_messages_socks_proxy() {
        let cli = parse(&["sunbeam", "build", "messages-socks-proxy"]);
        match cli.verb {
            Some(Verb::Build { what, .. }) => {
                assert!(matches!(what, BuildTarget::MessagesSocksProxy));
            }
            _ => panic!("expected Build"),
        }
    }

    // 20. test_hire_date_validation
    #[test]
    fn test_hire_date_valid() {
        let cli = parse(&[
            "sunbeam", "user", "onboard", "a@b.com",
            "--hire-date", "2026-01-15",
        ]);
        match cli.verb {
            Some(Verb::User { action: Some(UserAction::Onboard { hire_date, .. }) }) => {
                assert_eq!(hire_date, "2026-01-15");
            }
            _ => panic!("expected User Onboard"),
        }
    }

    #[test]
    fn test_hire_date_invalid() {
        let result = Cli::try_parse_from(&[
            "sunbeam", "user", "onboard", "a@b.com",
            "--hire-date", "not-a-date",
        ]);
        assert!(result.is_err());
    }

    // -- Workflow subcommand tests --

    #[test]
    fn test_workflow_list() {
        let cli = parse(&["sunbeam", "workflow", "list"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => {
                assert!(matches!(action, crate::workflows::cmd::WorkflowAction::List { .. }));
            }
            _ => panic!("expected Workflow List"),
        }
    }

    #[test]
    fn test_workflow_list_with_status_filter() {
        let cli = parse(&["sunbeam", "workflow", "list", "--status", "complete"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::List { status } => {
                    assert_eq!(status, "complete");
                }
                _ => panic!("expected List"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_status() {
        let cli = parse(&["sunbeam", "workflow", "status", "abc-123"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::Status { id } => {
                    assert_eq!(id, "abc-123");
                }
                _ => panic!("expected Status"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_retry() {
        let cli = parse(&["sunbeam", "workflow", "retry", "wf-456"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::Retry { id } => {
                    assert_eq!(id, "wf-456");
                }
                _ => panic!("expected Retry"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_cancel() {
        let cli = parse(&["sunbeam", "workflow", "cancel", "wf-789"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::Cancel { id } => {
                    assert_eq!(id, "wf-789");
                }
                _ => panic!("expected Cancel"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_run_default_file() {
        let cli = parse(&["sunbeam", "workflow", "run"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::Run { file } => {
                    assert_eq!(file, "");
                }
                _ => panic!("expected Run"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_run_with_file() {
        let cli = parse(&["sunbeam", "workflow", "run", "deploy.yaml"]);
        match cli.verb {
            Some(Verb::Workflow { action }) => match action {
                crate::workflows::cmd::WorkflowAction::Run { file } => {
                    assert_eq!(file, "deploy.yaml");
                }
                _ => panic!("expected Run"),
            },
            _ => panic!("expected Workflow"),
        }
    }

    #[test]
    fn test_workflow_status_missing_id() {
        let result = Cli::try_parse_from(&["sunbeam", "workflow", "status"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deploy_no_target() {
        let cli = parse(&["sunbeam", "deploy"]);
        match cli.verb {
            Some(Verb::Deploy { target, all }) => {
                assert!(target.is_none());
                assert!(!all);
            }
            _ => panic!("expected Deploy"),
        }
    }

    #[test]
    fn test_deploy_with_target() {
        let cli = parse(&["sunbeam", "deploy", "hydra"]);
        match cli.verb {
            Some(Verb::Deploy { target, .. }) => assert_eq!(target.unwrap(), "hydra"),
            _ => panic!("expected Deploy"),
        }
    }

    #[test]
    fn test_deploy_all() {
        let cli = parse(&["sunbeam", "deploy", "--all"]);
        match cli.verb {
            Some(Verb::Deploy { all, .. }) => assert!(all),
            _ => panic!("expected Deploy"),
        }
    }

    #[test]
    fn test_secrets_list() {
        let cli = parse(&["sunbeam", "secrets", "hydra"]);
        match cli.verb {
            Some(Verb::Secrets { service, action }) => {
                assert_eq!(service, "hydra");
                assert!(action.is_none());
            }
            _ => panic!("expected Secrets"),
        }
    }

    #[test]
    fn test_secrets_get() {
        let cli = parse(&["sunbeam", "secrets", "hydra", "get", "system-secret"]);
        match cli.verb {
            Some(Verb::Secrets { service, action }) => {
                assert_eq!(service, "hydra");
                match action {
                    Some(SecretsAction::Get { key }) => assert_eq!(key, "system-secret"),
                    _ => panic!("expected Get"),
                }
            }
            _ => panic!("expected Secrets"),
        }
    }

    #[test]
    fn test_shell() {
        let cli = parse(&["sunbeam", "shell", "postgres"]);
        match cli.verb {
            Some(Verb::Shell { service }) => assert_eq!(service, "postgres"),
            _ => panic!("expected Shell"),
        }
    }
}

/// Main dispatch function — parse CLI args and route to subcommands.
pub async fn dispatch() -> Result<()> {
    let cli = Cli::parse();

    // Resolve the active context from config + CLI flags (like kubectl)
    let config = crate::config::load_config();
    let active = crate::config::resolve_context(
        &config,
        "",
        cli.context.as_deref(),
        &cli.domain,
    );

    // Initialize kube context from the resolved context
    let kube_ctx_str = if active.kube_context.is_empty() {
        "sunbeam".to_string()
    } else {
        active.kube_context.clone()
    };
    let ssh_host_str = active.ssh_host.clone();
    crate::kube::set_context(&kube_ctx_str, &ssh_host_str);

    // Store active context globally for other modules to read
    crate::config::set_active_context(active);

    match cli.verb {
        None => {
            // Print help via clap
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }

        Some(Verb::Up) => {
            crate::output::step("Bringing up cluster (workflow engine)...");

            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::up::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let initial_data = serde_json::json!({
                "__ctx": step_ctx,
                "domain": "",
            });

            let instance = wfe::run_workflow_sync(
                &host,
                "up",
                2,
                initial_data,
                std::time::Duration::from_secs(3600),
            )
            .await
            .map_err(|e| SunbeamError::Other(format!("up workflow failed: {e}")))?;

            crate::workflows::up::print_summary(&instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::Other(format!(
                    "up workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Status { target }) => {
            crate::services::cmd_status(target.as_deref()).await
        }

        Some(Verb::Apply {
            namespace,
            apply_all,
            domain,
            email,
        }) => {
            let is_production = !crate::config::active_context().ssh_host.is_empty();
            let env_str = if is_production { "production" } else { "local" };
            let domain = if domain.is_empty() {
                cli.domain.clone()
            } else {
                domain
            };
            let email = if email.is_empty() {
                cli.email.clone()
            } else {
                email
            };
            let ns = namespace.unwrap_or_default();

            // Production full-apply requires --all or confirmation
            if is_production && ns.is_empty() && !apply_all {
                crate::output::warn(
                    "This will apply ALL namespaces to production.",
                );
                eprint!("  Continue? [y/N] ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            crate::manifests::cmd_apply(&env_str, &domain, &email, &ns).await
        }

        Some(Verb::Seed) => {
            crate::output::step("Seeding secrets (workflow engine)...");

            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::seed::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let initial_data = serde_json::json!({
                "__ctx": step_ctx,
            });

            let instance = wfe::run_workflow_sync(
                &host,
                "seed",
                2,
                initial_data,
                std::time::Duration::from_secs(900),
            )
            .await
            .map_err(|e| SunbeamError::secrets(format!("seed workflow failed: {e}")))?;

            crate::workflows::seed::print_summary(&instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::secrets(format!(
                    "seed workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Verify) => {
            crate::output::step("Verifying VSO -> OpenBao integration (workflow engine)...");

            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::verify::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let initial_data = serde_json::json!({
                "__ctx": step_ctx,
            });

            let instance = wfe::run_workflow_sync(
                &host,
                "verify",
                1,
                initial_data,
                std::time::Duration::from_secs(300),
            )
            .await
            .map_err(|e| SunbeamError::Other(format!("verify workflow failed: {e}")))?;

            crate::workflows::verify::print_summary(&instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::Other(format!(
                    "verify workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Logs { target, follow }) => {
            crate::services::cmd_logs(&target, follow).await
        }

        Some(Verb::Get { target, output }) => {
            crate::services::cmd_get(&target, &output).await
        }

        Some(Verb::Restart { target }) => {
            crate::services::cmd_restart(target.as_deref()).await
        }

        Some(Verb::Build { what, push, deploy, no_cache }) => {
            let push = push || deploy;
            crate::images::cmd_build(&what, push, deploy, no_cache).await
        }

        Some(Verb::Check { target }) => {
            crate::checks::cmd_check(target.as_deref()).await
        }

        Some(Verb::Mirror) => crate::images::cmd_mirror().await,

        Some(Verb::Bootstrap) => {
            crate::output::step("Bootstrapping Gitea (workflow engine)...");

            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };

            let host = crate::workflows::host::create_host(&ctx_name).await?;
            crate::workflows::bootstrap::register(&host).await;

            let step_ctx = crate::workflows::StepContext::from_active();
            let initial_data = serde_json::json!({
                "__ctx": step_ctx,
            });

            let instance = wfe::run_workflow_sync(
                &host,
                "bootstrap",
                1,
                initial_data,
                std::time::Duration::from_secs(300),
            )
            .await
            .map_err(|e| SunbeamError::Other(format!("bootstrap workflow failed: {e}")))?;

            crate::workflows::bootstrap::print_summary(&instance);
            crate::workflows::host::shutdown_host(host).await;

            if instance.status != wfe_core::models::WorkflowStatus::Complete {
                return Err(SunbeamError::Other(format!(
                    "bootstrap workflow ended with status {:?}",
                    instance.status
                )));
            }

            Ok(())
        }

        Some(Verb::Config { action }) => match action {
            None => {
                use clap::CommandFactory;
                // Print config subcommand help
                let mut cmd = Cli::command();
                let sub = cmd
                    .find_subcommand_mut("config")
                    .expect("config subcommand");
                sub.print_help()?;
                println!();
                Ok(())
            }
            Some(ConfigAction::Set {
                domain: set_domain,
                host,
                infra_dir,
                acme_email,
                context_name,
            }) => {
                let mut config = crate::config::load_config();
                // Determine which context to modify
                let ctx_name = if context_name.is_empty() {
                    if !config.current_context.is_empty() {
                        config.current_context.clone()
                    } else {
                        "production".to_string()
                    }
                } else {
                    context_name
                };

                let ctx = config.contexts.entry(ctx_name.clone()).or_default();
                if !set_domain.is_empty() {
                    ctx.domain = set_domain;
                }
                if !host.is_empty() {
                    ctx.ssh_host = host.clone();
                    config.production_host = host; // keep legacy field in sync
                }
                if !infra_dir.is_empty() {
                    ctx.infra_dir = infra_dir.clone();
                    config.infra_directory = infra_dir;
                }
                if !acme_email.is_empty() {
                    ctx.acme_email = acme_email.clone();
                    config.acme_email = acme_email;
                }
                if config.current_context.is_empty() {
                    config.current_context = ctx_name;
                }
                crate::config::save_config(&config)
            }
            Some(ConfigAction::UseContext { name }) => {
                let mut config = crate::config::load_config();
                if !config.contexts.contains_key(&name) {
                    crate::output::warn(&format!("Context '{name}' does not exist. Creating empty context."));
                    config.contexts.insert(name.clone(), crate::config::Context::default());
                }
                config.current_context = name.clone();
                crate::config::save_config(&config)?;
                crate::output::ok(&format!("Switched to context '{name}'."));
                Ok(())
            }
            Some(ConfigAction::Get) => {
                let config = crate::config::load_config();
                let current = if config.current_context.is_empty() {
                    "(none)"
                } else {
                    &config.current_context
                };
                crate::output::ok(&format!("Current context: {current}"));
                println!();
                for (name, ctx) in &config.contexts {
                    let marker = if name == current { " *" } else { "" };
                    crate::output::ok(&format!("Context: {name}{marker}"));
                    if !ctx.domain.is_empty() {
                        crate::output::ok(&format!("  domain:       {}", ctx.domain));
                    }
                    if !ctx.kube_context.is_empty() {
                        crate::output::ok(&format!("  kube-context: {}", ctx.kube_context));
                    }
                    if !ctx.ssh_host.is_empty() {
                        crate::output::ok(&format!("  ssh-host:     {}", ctx.ssh_host));
                    }
                    if !ctx.infra_dir.is_empty() {
                        crate::output::ok(&format!("  infra-dir:    {}", ctx.infra_dir));
                    }
                    if !ctx.acme_email.is_empty() {
                        crate::output::ok(&format!("  acme-email:   {}", ctx.acme_email));
                    }
                    println!();
                }
                Ok(())
            }
            Some(ConfigAction::Clear) => crate::config::clear_config(),
        },

        Some(Verb::K8s { kubectl_args }) => {
            crate::kube::cmd_k8s(&kubectl_args).await
        }

        Some(Verb::Bao { bao_args }) => {
            crate::kube::cmd_bao(&bao_args).await
        }

        Some(Verb::User { action }) => match action {
            None => {
                use clap::CommandFactory;
                let mut cmd = Cli::command();
                let sub = cmd
                    .find_subcommand_mut("user")
                    .expect("user subcommand");
                sub.print_help()?;
                println!();
                Ok(())
            }
            Some(UserAction::List { search }) => {
                crate::users::cmd_user_list(&search).await
            }
            Some(UserAction::Get { target }) => {
                crate::users::cmd_user_get(&target).await
            }
            Some(UserAction::Create {
                email,
                name,
                schema,
            }) => crate::users::cmd_user_create(&email, &name, &schema).await,
            Some(UserAction::Delete { target }) => {
                crate::users::cmd_user_delete(&target).await
            }
            Some(UserAction::Recover { target }) => {
                crate::users::cmd_user_recover(&target).await
            }
            Some(UserAction::Disable { target }) => {
                crate::users::cmd_user_disable(&target).await
            }
            Some(UserAction::Enable { target }) => {
                crate::users::cmd_user_enable(&target).await
            }
            Some(UserAction::SetPassword { target, password }) => {
                let pw = match password {
                    Some(p) => p,
                    None => {
                        eprint!("Password: ");
                        let mut pw = String::new();
                        std::io::stdin().read_line(&mut pw)?;
                        pw.trim().to_string()
                    }
                };
                crate::users::cmd_user_set_password(&target, &pw).await
            }
            Some(UserAction::Onboard {
                email,
                name,
                schema,
                no_email,
                notify,
                job_title,
                department,
                office_location,
                hire_date,
                manager,
            }) => {
                crate::users::cmd_user_onboard(
                    &email,
                    &name,
                    &schema,
                    !no_email,
                    &notify,
                    &job_title,
                    &department,
                    &office_location,
                    &hire_date,
                    &manager,
                )
                .await
            }
            Some(UserAction::Offboard { target }) => {
                crate::users::cmd_user_offboard(&target).await
            }
        },

        Some(Verb::Auth { action }) => match action {
            None => crate::auth::cmd_auth_status().await,
            Some(AuthAction::Login { domain }) => {
                crate::auth::cmd_auth_login_all(domain.as_deref()).await
            }
            Some(AuthAction::Sso { domain }) => {
                crate::auth::cmd_auth_sso_login(domain.as_deref()).await
            }
            Some(AuthAction::Git { domain }) => {
                crate::auth::cmd_auth_git_login(domain.as_deref()).await
            }
            Some(AuthAction::Logout) => crate::auth::cmd_auth_logout().await,
            Some(AuthAction::Status) => crate::auth::cmd_auth_status().await,
            Some(AuthAction::Token) => crate::auth::cmd_auth_token().await,
        },

        Some(Verb::Pm { action }) => match action {
            None => {
                use clap::CommandFactory;
                let mut cmd = Cli::command();
                let sub = cmd
                    .find_subcommand_mut("pm")
                    .expect("pm subcommand");
                sub.print_help()?;
                println!();
                Ok(())
            }
            Some(PmAction::List { source, state }) => {
                let src = if source == "all" { None } else { Some(source.as_str()) };
                crate::pm::cmd_pm_list(src, &state).await
            }
            Some(PmAction::Show { id }) => {
                crate::pm::cmd_pm_show(&id).await
            }
            Some(PmAction::Create { title, body, source, target }) => {
                crate::pm::cmd_pm_create(&title, &body, &source, &target).await
            }
            Some(PmAction::Comment { id, text }) => {
                crate::pm::cmd_pm_comment(&id, &text).await
            }
            Some(PmAction::Close { id }) => {
                crate::pm::cmd_pm_close(&id).await
            }
            Some(PmAction::Assign { id, user }) => {
                crate::pm::cmd_pm_assign(&id, &user).await
            }
        },

        Some(Verb::Workflow { action }) => {
            let ctx_name = {
                let cfg = crate::config::load_config();
                if cfg.current_context.is_empty() {
                    "default".to_string()
                } else {
                    cfg.current_context.clone()
                }
            };
            crate::workflows::cmd::dispatch(&ctx_name, action).await
        }

        Some(Verb::Deploy { target, all }) => {
            if all || target.is_none() {
                let is_production = !crate::config::active_context().ssh_host.is_empty();
                let env_str = if is_production { "production" } else { "local" };
                let domain = cli.domain.clone();
                let email = cli.email.clone();
                crate::manifests::cmd_apply(env_str, &domain, &email, "").await
            } else {
                let target = target.unwrap();
                crate::service_cmds::cmd_deploy(&target, &cli.domain, &cli.email).await
            }
        }

        Some(Verb::Secrets { service, action }) => {
            crate::service_cmds::cmd_secrets(&service, action).await
        }

        Some(Verb::Shell { service }) => {
            crate::service_cmds::cmd_shell(&service).await
        }

        Some(Verb::Connect) => crate::vpn_cmds::cmd_connect().await,

        Some(Verb::Disconnect) => crate::vpn_cmds::cmd_disconnect().await,

        Some(Verb::Vpn { action }) => match action {
            VpnAction::Status => crate::vpn_cmds::cmd_vpn_status().await,
        },

        Some(Verb::Update) => crate::update::cmd_update().await,

        Some(Verb::Version) => {
            crate::update::cmd_version();
            Ok(())
        }
    }
}
