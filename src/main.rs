use tracing::Instrument;

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Parse CLI early so we can initialize the subscriber with the right mode.
    let cli = <sunbeam_sdk::cli::Cli as clap::Parser>::parse();

    let level_override = if cli.quiet {
        Some("sunbeam=warn,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,warn")
    } else {
        match cli.verbose {
            0 => None,
            1 => Some("sunbeam=debug,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,warn"),
            _ => Some("sunbeam=trace,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,warn"),
        }
    };

    if let Err(e) = sunbeam_sdk::logging::init_subscriber(cli.log_mode, level_override) {
        eprintln!("Failed to initialize logger: {e}");
        std::process::exit(1);
    }

    // Log panics through tracing so they appear in all output modes.
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}");
    }));

    tracing::debug!("sunbeam starting, log_mode = {:?}", cli.log_mode);

    let root = tracing::info_span!("sunbeam");
    match sunbeam_sdk::cli::dispatch(cli).instrument(root).await {
        Ok(()) => {}
        Err(e) => {
            let code = e.exit_code();
            tracing::error!("{e}");

            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                tracing::debug!("caused by: {cause}");
                source = std::error::Error::source(cause);
            }

            std::process::exit(code);
        }
    }
}
