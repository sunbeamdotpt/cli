mod checks;
mod cli;
mod cluster;
mod config;
mod gitea;
mod images;
mod kube;
mod manifests;
mod openbao;
mod output;
mod secrets;
mod services;
mod tools;
mod update;
mod users;

use anyhow::Result;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("\nERROR: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    cli::dispatch().await
}
