use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "capsule", about = "Asynchronous zsh prompt engine", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Session worker started by zsh
    #[command(hide = true)]
    Worker,
    /// Set nonblocking flags on inherited shell endpoints
    #[command(hide = true)]
    FdConfig,
    /// Output shell initialization script
    Init {
        /// Target shell
        shell: Shell,
    },
    /// Output preset module definitions as TOML
    Preset,
}

#[derive(Clone, ValueEnum)]
pub enum Shell {
    /// zsh
    Zsh,
}
