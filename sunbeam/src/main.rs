mod cli;

#[tokio::main]
async fn main() {
    // Install rustls crypto provider (ring) before any TLS operations.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize tracing subscriber.
    // Respects RUST_LOG env var (e.g. RUST_LOG=debug, RUST_LOG=sunbeam=trace).
    // Default: warn for dependencies, info for sunbeam + sunbeam_sdk.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("sunbeam=info,sunbeam_sdk=info,warn")
            }),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    match cli::dispatch().await {
        Ok(()) => {}
        Err(e) => {
            let code = e.exit_code();
            tracing::error!("{e}");

            // Print source chain for non-trivial errors
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                tracing::debug!("caused by: {cause}");
                source = std::error::Error::source(cause);
            }

            std::process::exit(code);
        }
    }
}
