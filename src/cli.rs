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
    /// Push local state to hub (also captures live dotfile edits)
    Push {
        /// Target machine name (default: all)
        machine: Option<String>,
    },
    /// Capture live dotfile edits into repos and push fleet state
    Capture {
        /// Live files or dotfiles source paths to capture (empty = plain push)
        paths: Vec<std::path::PathBuf>,
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
    /// List trusted hub fingerprints (QUIC cert TOFU)
    List,
    /// Forget a trusted hub fingerprint (accept the new one on next connect)
    Rm {
        /// Hub address, e.g. 100.89.126.211:42069
        address: String,
    },
    /// SSH host-key trust (hub pulls)
    #[command(subcommand)]
    Ssh(TrustSshAction),
}

#[derive(Subcommand)]
pub enum TrustSshAction {
    /// List trusted SSH host-key fingerprints
    List,
    /// Forget a machine's SSH host-key fingerprint (re-trust via TOFU on next pull)
    Rm {
        /// Machine "host:port", e.g. 100.89.198.212:22
        host_port: String,
    },
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
