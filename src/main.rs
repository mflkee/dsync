mod cli;
mod client;
mod config;
mod hub;
mod protocol;
mod projects;
mod ssh;
mod zen;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dsync=info".into()),
        )
        .init();

    let _ = rustls::crypto::ring::default_provider().install_default();

    let args = cli::Cli::parse();
    let cfg = config::Config::load()?;

    match args.command {
        cli::Commands::Daemon => hub::run_server(cfg).await,
        cli::Commands::Push { machine } => client::push(cfg, machine).await,
        cli::Commands::Pull { machine } => client::pull(cfg, machine).await,
        cli::Commands::Status => client::status(cfg).await,
    }
}
