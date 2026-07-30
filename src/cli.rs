use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dsync", about = "Machine state synchronizer over QUIC")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start hub daemon (server mode)
    Daemon,
    /// Push local state to hub
    Push {
        /// Target machine name (default: all)
        machine: Option<String>,
    },
    /// Pull state from hub
    Pull {
        /// Source machine name (default: all)
        machine: Option<String>,
    },
    /// Show sync status
    Status,
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
