#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Parse CLI early so we can initialize the subscriber with the right mode.
    let cli = <sunbeam_sdk::cli::Cli as clap::Parser>::parse();

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
        eprintln!("Failed to initialize logger: {e}");
        std::process::exit(1);
    }

    // Log panics through tracing so they appear in all output modes.
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(msg = "panic", info = %info);
    }));

    tracing::debug!(msg = "sunbeam starting", log_mode = ?cli.log_mode);

    match sunbeam_sdk::cli::dispatch(cli).await {
        Ok(()) => {}
        Err(e) => {
            let code = e.exit_code();
            tracing::error!(msg = "command failed", err = %e);

            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                tracing::debug!(msg = "caused by", err = %cause);
                source = std::error::Error::source(cause);
            }

            std::process::exit(code);
        }
    }
}
