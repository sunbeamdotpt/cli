use sunbeam_sdk::error::Result;
use sunbeam_sdk::images::BuildTarget;
use sunbeam_sdk::output::OutputFormat;
use clap::{Parser, Subcommand};

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

    /// Output format (json, yaml, table). Default: table.
    #[arg(short = 'o', long = "output", global = true, default_value = "table")]
    pub output_format: OutputFormat,

    #[command(subcommand)]
    pub verb: Option<Verb>,
}


#[derive(Subcommand, Debug)]
pub enum Verb {
    // -- Infrastructure commands (preserved) ----------------------------------

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
        /// kubectl output format (yaml, json, wide).
        #[arg(long = "kubectl-output", default_value = "yaml", value_parser = ["yaml", "json", "wide"])]
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

    /// Project management across Planka and Gitea.
    Pm {
        #[command(subcommand)]
        action: Option<PmAction>,
    },

    /// Self-update from latest mainline commit.
    Update,

    /// Print version info.
    Version,

    // -- Service commands (new) -----------------------------------------------

    /// Authentication, identity & OAuth2 management.
    Auth {
        #[command(subcommand)]
        action: sunbeam_sdk::identity::cli::AuthCommand,
    },

    /// Version control (Gitea).
    Vcs {
        #[command(subcommand)]
        action: sunbeam_sdk::gitea::cli::VcsCommand,
    },

    /// Chat / messaging (Matrix).
    Chat {
        #[command(subcommand)]
        action: sunbeam_sdk::matrix::cli::ChatCommand,
    },

    /// Search engine (OpenSearch).
    Search {
        #[command(subcommand)]
        action: sunbeam_sdk::search::cli::SearchCommand,
    },

    /// Object storage (S3).
    Storage {
        #[command(subcommand)]
        action: sunbeam_sdk::storage::cli::StorageCommand,
    },

    /// Media / video (LiveKit).
    Media {
        #[command(subcommand)]
        action: sunbeam_sdk::media::cli::MediaCommand,
    },

    /// Monitoring (Prometheus, Loki, Grafana).
    Mon {
        #[command(subcommand)]
        action: sunbeam_sdk::monitoring::cli::MonCommand,
    },

    /// Secrets management (OpenBao/Vault).
    Vault {
        #[command(subcommand)]
        action: sunbeam_sdk::openbao::cli::VaultCommand,
    },

    /// People / contacts (La Suite).
    People {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::PeopleCommand,
    },

    /// Documents (La Suite).
    Docs {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::DocsCommand,
    },

    /// Video meetings (La Suite).
    Meet {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::MeetCommand,
    },

    /// File storage (La Suite).
    Drive {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::DriveCommand,
    },

    /// Email (La Suite).
    Mail {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::MailCommand,
    },

    /// Calendar (La Suite).
    Cal {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::CalCommand,
    },

    /// Search across La Suite services.
    Find {
        #[command(subcommand)]
        action: sunbeam_sdk::lasuite::cli::FindCommand,
    },
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
        let cli = parse(&["sunbeam", "get", "ory/kratos-abc", "--kubectl-output", "json"]);
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

    // -- New service subcommand tests -----------------------------------------

    #[test]
    fn test_auth_identity_list() {
        let cli = parse(&["sunbeam", "auth", "identity", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Auth { .. })));
    }

    #[test]
    fn test_auth_login() {
        let cli = parse(&["sunbeam", "auth", "login"]);
        assert!(matches!(cli.verb, Some(Verb::Auth { .. })));
    }

    #[test]
    fn test_vcs_repo_search() {
        let cli = parse(&["sunbeam", "vcs", "repo", "search", "-q", "cli"]);
        assert!(matches!(cli.verb, Some(Verb::Vcs { .. })));
    }

    #[test]
    fn test_vcs_issue_list() {
        let cli = parse(&["sunbeam", "vcs", "issue", "list", "-r", "studio/cli"]);
        assert!(matches!(cli.verb, Some(Verb::Vcs { .. })));
    }

    #[test]
    fn test_chat_whoami() {
        let cli = parse(&["sunbeam", "chat", "whoami"]);
        assert!(matches!(cli.verb, Some(Verb::Chat { .. })));
    }

    #[test]
    fn test_chat_room_list() {
        let cli = parse(&["sunbeam", "chat", "room", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Chat { .. })));
    }

    #[test]
    fn test_search_cluster_health() {
        let cli = parse(&["sunbeam", "search", "cluster", "health"]);
        assert!(matches!(cli.verb, Some(Verb::Search { .. })));
    }

    #[test]
    fn test_storage_bucket_list() {
        let cli = parse(&["sunbeam", "storage", "bucket", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Storage { .. })));
    }

    #[test]
    fn test_media_room_list() {
        let cli = parse(&["sunbeam", "media", "room", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Media { .. })));
    }

    #[test]
    fn test_mon_prometheus_query() {
        let cli = parse(&["sunbeam", "mon", "prometheus", "query", "-q", "up"]);
        assert!(matches!(cli.verb, Some(Verb::Mon { .. })));
    }

    #[test]
    fn test_mon_grafana_dashboard_list() {
        let cli = parse(&["sunbeam", "mon", "grafana", "dashboard", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Mon { .. })));
    }

    #[test]
    fn test_vault_status() {
        let cli = parse(&["sunbeam", "vault", "status"]);
        assert!(matches!(cli.verb, Some(Verb::Vault { .. })));
    }

    #[test]
    fn test_people_contact_list() {
        let cli = parse(&["sunbeam", "people", "contact", "list"]);
        assert!(matches!(cli.verb, Some(Verb::People { .. })));
    }

    #[test]
    fn test_docs_document_list() {
        let cli = parse(&["sunbeam", "docs", "document", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Docs { .. })));
    }

    #[test]
    fn test_meet_room_list() {
        let cli = parse(&["sunbeam", "meet", "room", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Meet { .. })));
    }

    #[test]
    fn test_drive_file_list() {
        let cli = parse(&["sunbeam", "drive", "file", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Drive { .. })));
    }

    #[test]
    fn test_mail_mailbox_list() {
        let cli = parse(&["sunbeam", "mail", "mailbox", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Mail { .. })));
    }

    #[test]
    fn test_cal_calendar_list() {
        let cli = parse(&["sunbeam", "cal", "calendar", "list"]);
        assert!(matches!(cli.verb, Some(Verb::Cal { .. })));
    }

    #[test]
    fn test_find_search() {
        let cli = parse(&["sunbeam", "find", "search", "-q", "hello"]);
        assert!(matches!(cli.verb, Some(Verb::Find { .. })));
    }

    #[test]
    fn test_global_output_format() {
        let cli = parse(&["sunbeam", "-o", "json", "vault", "status"]);
        assert!(matches!(cli.output_format, OutputFormat::Json));
        assert!(matches!(cli.verb, Some(Verb::Vault { .. })));
    }

    #[test]
    fn test_infra_commands_preserved() {
        // Verify all old infra commands still parse
        assert!(matches!(parse(&["sunbeam", "up"]).verb, Some(Verb::Up)));
        assert!(matches!(parse(&["sunbeam", "seed"]).verb, Some(Verb::Seed)));
        assert!(matches!(parse(&["sunbeam", "verify"]).verb, Some(Verb::Verify)));
        assert!(matches!(parse(&["sunbeam", "mirror"]).verb, Some(Verb::Mirror)));
        assert!(matches!(parse(&["sunbeam", "bootstrap"]).verb, Some(Verb::Bootstrap)));
        assert!(matches!(parse(&["sunbeam", "update"]).verb, Some(Verb::Update)));
        assert!(matches!(parse(&["sunbeam", "version"]).verb, Some(Verb::Version)));
    }
}

/// Main dispatch function — parse CLI args and route to subcommands.
pub async fn dispatch() -> Result<()> {
    let cli = Cli::parse();

    // Resolve the active context from config + CLI flags (like kubectl)
    let config = sunbeam_sdk::config::load_config();
    let active = sunbeam_sdk::config::resolve_context(
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
    sunbeam_sdk::kube::set_context(&kube_ctx_str, &ssh_host_str);

    // Store active context globally for other modules to read
    sunbeam_sdk::config::set_active_context(active);

    match cli.verb {
        None => {
            // Print help via clap
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }

        Some(Verb::Up) => sunbeam_sdk::cluster::cmd_up().await,

        Some(Verb::Status { target }) => {
            sunbeam_sdk::services::cmd_status(target.as_deref()).await
        }

        Some(Verb::Apply {
            namespace,
            apply_all,
            domain,
            email,
        }) => {
            let is_production = !sunbeam_sdk::config::active_context().ssh_host.is_empty();
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
                sunbeam_sdk::output::warn(
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

            sunbeam_sdk::manifests::cmd_apply(&env_str, &domain, &email, &ns).await
        }

        Some(Verb::Seed) => sunbeam_sdk::secrets::cmd_seed().await,

        Some(Verb::Verify) => sunbeam_sdk::secrets::cmd_verify().await,

        Some(Verb::Logs { target, follow }) => {
            sunbeam_sdk::services::cmd_logs(&target, follow).await
        }

        Some(Verb::Get { target, output }) => {
            sunbeam_sdk::services::cmd_get(&target, &output).await
        }

        Some(Verb::Restart { target }) => {
            sunbeam_sdk::services::cmd_restart(target.as_deref()).await
        }

        Some(Verb::Build { what, push, deploy, no_cache }) => {
            let push = push || deploy;
            sunbeam_sdk::images::cmd_build(&what, push, deploy, no_cache).await
        }

        Some(Verb::Check { target }) => {
            sunbeam_sdk::checks::cmd_check(target.as_deref()).await
        }

        Some(Verb::Mirror) => sunbeam_sdk::images::cmd_mirror().await,

        Some(Verb::Bootstrap) => sunbeam_sdk::gitea::cmd_bootstrap().await,

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
                let mut config = sunbeam_sdk::config::load_config();
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
                sunbeam_sdk::config::save_config(&config)
            }
            Some(ConfigAction::UseContext { name }) => {
                let mut config = sunbeam_sdk::config::load_config();
                if !config.contexts.contains_key(&name) {
                    sunbeam_sdk::output::warn(&format!("Context '{name}' does not exist. Creating empty context."));
                    config.contexts.insert(name.clone(), sunbeam_sdk::config::Context::default());
                }
                config.current_context = name.clone();
                sunbeam_sdk::config::save_config(&config)?;
                sunbeam_sdk::output::ok(&format!("Switched to context '{name}'."));
                Ok(())
            }
            Some(ConfigAction::Get) => {
                let config = sunbeam_sdk::config::load_config();
                let current = if config.current_context.is_empty() {
                    "(none)"
                } else {
                    &config.current_context
                };
                sunbeam_sdk::output::ok(&format!("Current context: {current}"));
                println!();
                for (name, ctx) in &config.contexts {
                    let marker = if name == current { " *" } else { "" };
                    sunbeam_sdk::output::ok(&format!("Context: {name}{marker}"));
                    if !ctx.domain.is_empty() {
                        sunbeam_sdk::output::ok(&format!("  domain:       {}", ctx.domain));
                    }
                    if !ctx.kube_context.is_empty() {
                        sunbeam_sdk::output::ok(&format!("  kube-context: {}", ctx.kube_context));
                    }
                    if !ctx.ssh_host.is_empty() {
                        sunbeam_sdk::output::ok(&format!("  ssh-host:     {}", ctx.ssh_host));
                    }
                    if !ctx.infra_dir.is_empty() {
                        sunbeam_sdk::output::ok(&format!("  infra-dir:    {}", ctx.infra_dir));
                    }
                    if !ctx.acme_email.is_empty() {
                        sunbeam_sdk::output::ok(&format!("  acme-email:   {}", ctx.acme_email));
                    }
                    println!();
                }
                Ok(())
            }
            Some(ConfigAction::Clear) => sunbeam_sdk::config::clear_config(),
        },

        Some(Verb::K8s { kubectl_args }) => {
            sunbeam_sdk::kube::cmd_k8s(&kubectl_args).await
        }

        Some(Verb::Bao { bao_args }) => {
            sunbeam_sdk::kube::cmd_bao(&bao_args).await
        }

        Some(Verb::Auth { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::identity::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::Vcs { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::gitea::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::Chat { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::matrix::cli::dispatch(&sc, cli.output_format, action).await
        }

        Some(Verb::Search { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::search::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::Storage { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::storage::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::Media { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::media::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::Mon { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::monitoring::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::Vault { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::openbao::cli::dispatch(action, &sc, cli.output_format).await
        }

        Some(Verb::People { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_people(action, &sc, cli.output_format).await
        }

        Some(Verb::Docs { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_docs(action, &sc, cli.output_format).await
        }

        Some(Verb::Meet { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_meet(action, &sc, cli.output_format).await
        }

        Some(Verb::Drive { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_drive(action, &sc, cli.output_format).await
        }

        Some(Verb::Mail { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_mail(action, &sc, cli.output_format).await
        }

        Some(Verb::Cal { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_cal(action, &sc, cli.output_format).await
        }

        Some(Verb::Find { action }) => {
            let sc = sunbeam_sdk::client::SunbeamClient::from_context(
                &sunbeam_sdk::config::active_context(),
            );
            sunbeam_sdk::lasuite::cli::dispatch_find(action, &sc, cli.output_format).await
        }

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
                sunbeam_sdk::pm::cmd_pm_list(src, &state).await
            }
            Some(PmAction::Show { id }) => {
                sunbeam_sdk::pm::cmd_pm_show(&id).await
            }
            Some(PmAction::Create { title, body, source, target }) => {
                sunbeam_sdk::pm::cmd_pm_create(&title, &body, &source, &target).await
            }
            Some(PmAction::Comment { id, text }) => {
                sunbeam_sdk::pm::cmd_pm_comment(&id, &text).await
            }
            Some(PmAction::Close { id }) => {
                sunbeam_sdk::pm::cmd_pm_close(&id).await
            }
            Some(PmAction::Assign { id, user }) => {
                sunbeam_sdk::pm::cmd_pm_assign(&id, &user).await
            }
        },

        Some(Verb::Update) => sunbeam_sdk::update::cmd_update().await,

        Some(Verb::Version) => {
            sunbeam_sdk::update::cmd_version();
            Ok(())
        }
    }
}
