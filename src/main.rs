use std::io::IsTerminal;

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = <sunbeam_sdk::cli::Cli as clap::Parser>::parse();

    // Keep a tracing subscriber as a fallback for any remaining tracing::
    // calls in the codebase until the migration is fully complete.
    let base_filter = "sunbeam=info,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,kube_client::client::builder=off,warn";
    let level_override = if cli.quiet {
        Some(format!("sunbeam=warn,{}", &base_filter["sunbeam=info,".len()..]))
    } else {
        match cli.verbose {
            0 => None,
            1 => Some(format!("sunbeam=debug,{}", &base_filter["sunbeam=info,".len()..])),
            _ => Some(format!("sunbeam=trace,{}", &base_filter["sunbeam=info,".len()..])),
        }
    };
    if let Err(e) = sunbeam_sdk::logging::init_subscriber(cli.log_mode, level_override.as_deref()) {
        eprintln!("Failed to initialize fallback logger: {e}");
        std::process::exit(1);
    }

    let min_level = if cli.quiet {
        sunbeam_sdk::logger::Level::Warn
    } else {
        match cli.verbose {
            0 => sunbeam_sdk::logger::Level::Info,
            1 => sunbeam_sdk::logger::Level::Debug,
            _ => sunbeam_sdk::logger::Level::Trace,
        }
    };

    let logger = match cli.log_mode {
        sunbeam_sdk::logging::LogMode::Line => {
            sunbeam_sdk::logger::Logger::new(
                sunbeam_sdk::logger::LineSink::new().with_level(min_level),
            )
        }
        sunbeam_sdk::logging::LogMode::Json => {
            sunbeam_sdk::logger::Logger::new(
                sunbeam_sdk::logger::JsonSink::new().with_level(min_level),
            )
        }
        sunbeam_sdk::logging::LogMode::Threaded => {
            if std::io::stderr().is_terminal() {
                sunbeam_sdk::logger::Logger::new(
                    sunbeam_sdk::logger::ThreadedSink::new().with_level(min_level),
                )
            } else {
                sunbeam_sdk::logger::Logger::new(
                    sunbeam_sdk::logger::LineSink::new().with_level(min_level),
                )
            }
        }
    };

    std::panic::set_hook(Box::new(|info| {
        eprintln!("panic: {info}");
    }));

    sunbeam_sdk::debug!(logger, "sunbeam starting", log_mode = format!("{:?}", cli.log_mode));

    match sunbeam_sdk::cli::dispatch(&logger, cli).await {
        Ok(()) => {}
        Err(e) => {
            let code = e.exit_code();
            sunbeam_sdk::error!(logger, "command failed", err = format!("{e}"));

            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                sunbeam_sdk::debug!(logger, "caused by", err = format!("{cause}"));
                source = std::error::Error::source(cause);
            }

            std::process::exit(code);
        }
    }
}
