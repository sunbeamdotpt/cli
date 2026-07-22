#![deny(unused_mut)]
#![deny(clippy::missing_safety_doc)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
// just keeps syntax consistent
#![deny(clippy::needless_borrow)]

use std::io::IsTerminal;

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

    // Keep a tracing subscriber as a fallback for any remaining tracing::
    // calls in the codebase until the migration is fully complete.
    let base_filter = "sunbeam=info,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,kube_client::client::builder=off,warn";
    let level_override = if cli.quiet {
        Some(format!(
            "sunbeam=warn,{}",
            &base_filter["sunbeam=info,".len()..]
        ))
    } else {
        match cli.verbose {
            0 => None,
            1 => Some(format!(
                "sunbeam=debug,{}",
                &base_filter["sunbeam=info,".len()..]
            )),
            _ => Some(format!(
                "sunbeam=trace,{}",
                &base_filter["sunbeam=info,".len()..]
            )),
        }
    };
    if let Err(e) = sdk::logging::init_subscriber(cli.log_mode, level_override.as_deref()) {
        eprintln!("Failed to initialize fallback logger: {e}");
        std::process::exit(1);
    }

    let min_level = if cli.quiet {
        sdk::logger::Level::Warn
    } else {
        match cli.verbose {
            0 => sdk::logger::Level::Info,
            1 => sdk::logger::Level::Debug,
            _ => sdk::logger::Level::Trace,
        }
    };

    let logger = match cli.log_mode {
        sdk::logging::LogMode::Line => {
            sdk::logger::Logger::new(sdk::logger::LineSink::new().with_level(min_level))
        }
        sdk::logging::LogMode::Json => {
            sdk::logger::Logger::new(sdk::logger::JsonSink::new().with_level(min_level))
        }
        sdk::logging::LogMode::Threaded => {
            if std::io::stderr().is_terminal() {
                sdk::logger::Logger::new(sdk::logger::ThreadedSink::new().with_level(min_level))
            } else {
                sdk::logger::Logger::new(sdk::logger::LineSink::new().with_level(min_level))
            }
        }
    };

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
