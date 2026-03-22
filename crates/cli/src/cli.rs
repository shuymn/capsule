use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "capsule", about = "macOS zsh prompt engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Daemon management
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
    },
    /// Connect to the daemon (coproc relay)
    Connect,
    /// Output shell initialization script
    Init {
        /// Target shell
        shell: Shell,
    },
    /// Output preset module definitions as TOML
    Preset,
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Install the launchd plist and load the daemon service
    Install,
    /// Uninstall the launchd service and remove the plist
    Uninstall,
    /// Show daemon metrics and status
    Status {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, ValueEnum)]
pub enum Shell {
    /// zsh
    Zsh,
}
