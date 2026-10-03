//! CLI entry point for capsule, an asynchronous zsh prompt engine.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

mod cli;
mod pipe;
mod preset;
mod worker;

use clap::Parser;

use crate::cli::{Cli, Command, Shell};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Worker => worker::run(),
        Command::FdConfig => Ok(pipe::configure_shell_endpoints()?),
        Command::Init { shell: Shell::Zsh } => {
            print!("{}", capsule_core::init::zsh::generate());
            Ok(())
        }
        Command::Preset => preset::run(),
    }
}
