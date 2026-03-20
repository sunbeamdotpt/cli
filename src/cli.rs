use crate::error::{Result, SunbeamError};
use clap::{Parser, Subcommand, ValueEnum};

/// Sunbeam local dev stack manager.
#[derive(Parser, Debug)]
#[command(name = "sunbeam", about = "Sunbeam local dev stack manager")]
pub struct Cli {
    /// Target environment.
    #[arg(long, default_value = "local")]
    pub env: Env,

    /// kubectl context override.
    #[arg(long)]
    pub context: Option<String>,

    /// Domain suffix for production deploys (e.g. sunbeam.pt).
    #[arg(long, default_value = "")]
    pub domain: String,

    /// ACME email for cert-manager (e.g. ops@sunbeam.pt).
    #[arg(long, default_value = "")]
    pub email: String,

    #[command(subcommand)]
    pub verb: Option<Verb>,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Env {
    Local,
    Production,
}

impl std::fmt::Display for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Env::Local => write!(f, "local"),
            Env::Production => write!(f, "production"),
        }
    }
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

    /// Self-update from latest mainline commit.
    Update,

    /// Print version info.
    Version,
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// Log in via browser (OAuth2 authorization code flow).
    Login,
    /// Log out (remove cached tokens).
    Logout,
    /// Show current authentication status.
    Status,
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
        };
        write!(f, "{s}")
    }
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Set configuration values.
    Set {
        /// Production SSH host (e.g. user@server.example.com).
        #[arg(long, default_value = "")]
        host: String,
        /// Infrastructure directory root.
        #[arg(long, default_value = "")]
        infra_dir: String,
        /// ACME email for Let's Encrypt certificates.
        #[arg(long, default_value = "")]
        acme_email: String,
    },
    /// Get current configuration.
    Get,
    /// Clear configuration.
    Clear,
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

/// Default kubectl context per environment.
fn default_context(env: &Env) -> &'static str {
    match env {
        Env::Local => "sunbeam",
        Env::Production => "production",
    }
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
            Some(Verb::Build { what, push, deploy }) => {
                assert!(matches!(what, BuildTarget::Proxy));
                assert!(!push);
                assert!(!deploy);
            }
            _ => panic!("expected Build"),
        }
    }

    // 7. test_build_deploy_flag
    #[test]
    fn test_build_deploy_flag() {
        let cli = parse(&["sunbeam", "build", "proxy", "--deploy"]);
        match cli.verb {
            Some(Verb::Build { deploy, push, .. }) => {
                assert!(deploy);
                // clap does not imply --push; that logic is in dispatch()
                assert!(!push);
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
}

/// Main dispatch function — parse CLI args and route to subcommands.
pub async fn dispatch() -> Result<()> {
    let cli = Cli::parse();

    let ctx = cli
        .context
        .as_deref()
        .unwrap_or_else(|| default_context(&cli.env));

    // For production, resolve SSH host
    let ssh_host = match cli.env {
        Env::Production => {
            let host = crate::config::get_production_host();
            if host.is_empty() {
                return Err(SunbeamError::config(
                    "Production host not configured. \
                     Use `sunbeam config set --host` or set SUNBEAM_SSH_HOST.",
                ));
            }
            Some(host)
        }
        Env::Local => None,
    };

    // Initialize kube context
    crate::kube::set_context(ctx, ssh_host.as_deref().unwrap_or(""));

    match cli.verb {
        None => {
            // Print help via clap
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }

        Some(Verb::Up) => crate::cluster::cmd_up().await,

        Some(Verb::Status { target }) => {
            crate::services::cmd_status(target.as_deref()).await
        }

        Some(Verb::Apply {
            namespace,
            apply_all,
            domain,
            email,
        }) => {
            let env_str = cli.env.to_string();
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
            if matches!(cli.env, Env::Production) && ns.is_empty() && !apply_all {
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

        Some(Verb::Seed) => crate::secrets::cmd_seed().await,

        Some(Verb::Verify) => crate::secrets::cmd_verify().await,

        Some(Verb::Logs { target, follow }) => {
            crate::services::cmd_logs(&target, follow).await
        }

        Some(Verb::Get { target, output }) => {
            crate::services::cmd_get(&target, &output).await
        }

        Some(Verb::Restart { target }) => {
            crate::services::cmd_restart(target.as_deref()).await
        }

        Some(Verb::Build { what, push, deploy }) => {
            let push = push || deploy;
            crate::images::cmd_build(&what, push, deploy).await
        }

        Some(Verb::Check { target }) => {
            crate::checks::cmd_check(target.as_deref()).await
        }

        Some(Verb::Mirror) => crate::images::cmd_mirror().await,

        Some(Verb::Bootstrap) => crate::gitea::cmd_bootstrap().await,

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
                host,
                infra_dir,
                acme_email,
            }) => {
                let mut config = crate::config::load_config();
                if !host.is_empty() {
                    config.production_host = host;
                }
                if !infra_dir.is_empty() {
                    config.infra_directory = infra_dir;
                }
                if !acme_email.is_empty() {
                    config.acme_email = acme_email;
                }
                crate::config::save_config(&config)
            }
            Some(ConfigAction::Get) => {
                let config = crate::config::load_config();
                let host_display = if config.production_host.is_empty() {
                    "(not set)"
                } else {
                    &config.production_host
                };
                let infra_display = if config.infra_directory.is_empty() {
                    "(not set)"
                } else {
                    &config.infra_directory
                };
                let email_display = if config.acme_email.is_empty() {
                    "(not set)"
                } else {
                    &config.acme_email
                };
                crate::output::ok(&format!("Production host: {host_display}"));
                crate::output::ok(&format!(
                    "Infrastructure directory: {infra_display}"
                ));
                crate::output::ok(&format!("ACME email: {email_display}"));

                let effective = crate::config::get_production_host();
                if !effective.is_empty() {
                    crate::output::ok(&format!(
                        "Effective production host: {effective}"
                    ));
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
            None => {
                crate::auth::cmd_auth_status().await
            }
            Some(AuthAction::Login) => crate::auth::cmd_auth_login().await,
            Some(AuthAction::Logout) => crate::auth::cmd_auth_logout().await,
            Some(AuthAction::Status) => crate::auth::cmd_auth_status().await,
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

        Some(Verb::Update) => crate::update::cmd_update().await,

        Some(Verb::Version) => {
            crate::update::cmd_version();
            Ok(())
        }
    }
}
