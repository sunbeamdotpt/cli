use anyhow::{bail, Result};
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

    /// Self-update from latest mainline commit.
    Update,

    /// Print version info.
    Version,
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
        /// New password.
        password: String,
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

fn validate_date(s: &str) -> Result<String, String> {
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
                bail!(
                    "Production host not configured. \
                     Use `sunbeam config set --host` or set SUNBEAM_SSH_HOST."
                );
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
                crate::users::cmd_user_set_password(&target, &password).await
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

        Some(Verb::Update) => crate::update::cmd_update().await,

        Some(Verb::Version) => {
            crate::update::cmd_version();
            Ok(())
        }
    }
}
