mod bot;
mod cli;
mod client;
mod config;
mod doctor;
mod hub;
mod protocol;
mod projects;
mod ssh;
mod tui;

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
        cli::Commands::Push { machine } => Ok(print_lines(client::push(cfg, machine).await?)),
        cli::Commands::Pull { machine } => Ok(print_lines(client::pull(cfg, machine).await?)),
        cli::Commands::Status => Ok(print_lines(client::status(cfg).await?)),
        cli::Commands::Doctor => doctor::run(cfg).await,
        cli::Commands::Bot => bot::run(cfg).await,
        cli::Commands::Tui => tui::run(cfg),
    }
}

/// Печатает строки результата (push/pull/status возвращают линии вместо
/// печати в stdout — так те же функции безопасно вызываются из TUI).
fn print_lines(lines: Vec<String>) {
    for l in lines {
        println!("{l}");
    }
}
