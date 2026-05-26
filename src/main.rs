#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Parse CLI early so we can initialize the subscriber with the right mode.
    let cli = <sunbeam_sdk::cli::Cli as clap::Parser>::parse();

    if let Err(e) = sunbeam_sdk::logging::init_subscriber(cli.log_mode) {
        eprintln!("Failed to initialize logger: {e}");
        std::process::exit(1);
    }

    match sunbeam_sdk::cli::dispatch(cli).await {
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
