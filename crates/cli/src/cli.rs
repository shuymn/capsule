use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "capsule", about = "macOS zsh prompt engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the prompt daemon
    Daemon,
    /// Connect to the daemon (coproc relay)
    Connect,
    /// Output shell initialization script
    Init {
        /// Target shell
        shell: Shell,
    },
}

#[derive(Clone, ValueEnum)]
pub enum Shell {
    /// zsh
    Zsh,
}
