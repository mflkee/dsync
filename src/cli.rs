use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dsync", about = "Machine state synchronizer over QUIC")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Interactive first-run setup wizard
    Init,
    /// Start the hub daemon (QUIC server + SSH-pull coordinator)
    #[command(alias = "daemon")]
    Hub,
    /// Background sync loop: poll hub, push + pull every N seconds
    Watch {
        /// Poll interval in seconds (default: 900, i.e. 15 minutes)
        #[arg(long, default_value_t = 900)]
        interval: u64,
    },
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
    /// Manage trusted hub fingerprints (TOFU known_hosts)
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
    /// Show sync status
    Status,
    /// Run diagnostics
    Doctor,
    /// Start Telegram bot
    Bot,
    /// Start interactive TUI
    Tui,
}

#[derive(Subcommand)]
pub enum TrustAction {
    /// List trusted hub fingerprints
    List,
    /// Forget a trusted hub fingerprint (accept the new one on next connect)
    Rm {
        /// Hub address, e.g. 100.89.126.211:42069
        address: String,
    },
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
