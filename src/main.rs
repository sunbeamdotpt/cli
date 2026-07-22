#![deny(unused_mut)]
#![deny(clippy::missing_safety_doc)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
// just keeps syntax consistent
#![deny(clippy::needless_borrow)]

mod auth;
mod checks;
mod cli;
mod cluster;
mod describe;
mod discovery;
mod doctor;
mod down;
mod exec;
mod kanban;
mod operations;
mod output;
mod port_forward;
mod profiles_cli;
mod project;
mod registry;
mod secrets_cli;
mod service_cmds;
mod services;
mod topo;
mod update;
mod users;
mod wfectl;
mod workflows;
mod workflows_cmd;

#[tokio::main]
async fn main() {
    if let Err(e) = rustls::crypto::aws_lc_rs::default_provider().install_default() {
        eprintln!("Failed to install rustls crypto provider: {e:?}");
        std::process::exit(1);
    }

    let cli = <cli::Cli as clap::Parser>::parse();

    // Logging is opt-in: the default level is WARN so stdout/stderr stay
    // script-safe; -v/-vv/-vvv raise sunbeam+sdk to info/debug/trace and
    // --quiet drops to error. RUST_LOG overrides everything (handled inside
    // init_subscriber). Both the sdk Logger macros (via TracingSink) and
    // direct tracing:: calls flow through this one subscriber, which renders
    // the --log-mode line/json/threaded formats to stderr.
    let base = "tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,kube_client::client::builder=off,warn";
    let level = if cli.quiet {
        "error"
    } else {
        match cli.verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
    };
    let filter = format!("sunbeam={level},sdk={level},{base}");
    if let Err(e) = sdk::logging::init_subscriber(cli.log_mode, Some(&filter)) {
        eprintln!("Failed to initialize logging: {e}");
        std::process::exit(1);
    }

    let logger = sdk::logger::Logger::new(sdk::logger::TracingSink);

    std::panic::set_hook(Box::new(|info| {
        eprintln!("panic: {info}");
    }));

    sdk::debug!(
        logger,
        "sunbeam starting",
        log_mode = format!("{:?}", cli.log_mode)
    );

    match cli::dispatch(&logger, cli).await {
        Ok(()) => {}
        Err(e) => {
            let code = e.exit_code();
            sdk::error!(logger, "command failed", err = format!("{e}"));

            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                sdk::debug!(logger, "caused by", err = format!("{cause}"));
                source = std::error::Error::source(cause);
            }

            std::process::exit(code);
        }
    }
}
