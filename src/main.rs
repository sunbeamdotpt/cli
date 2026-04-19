#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Default log filter:
    //   sunbeam=info        — our own logs at INFO
    //   warn                — third-party crates at WARN+ by default
    //   tonic/hyper/h2/tower/reqwest=off — transport-layer crates are silenced.
    //     During cluster bootstrap the workflow engine's event publisher
    //     repeatedly hits wfe-server before it's up, causing 5-7 `ERROR
    //     client error (Connect)` lines from tonic per apply. Those errors
    //     are harmless (the event buffer absorbs them) and their volume
    //     buries real errors. Users who want to debug transport failures
    //     can override via `RUST_LOG=tonic=debug,…`.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "sunbeam=info,tonic=off,hyper=off,h2=off,tower=off,reqwest=off,kube_client::client::tls=off,warn",
                )
            }),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    match sunbeam_sdk::cli::dispatch().await {
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
