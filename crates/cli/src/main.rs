//! CLI entry point for capsule, a macOS zsh prompt engine.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

mod build_id;
mod cli;
mod connect;
mod daemon;

use clap::Parser;

use crate::cli::{Cli, Command, Shell};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon => daemon::run(),
        Command::Connect => connect::run(),
        Command::Init { shell } => {
            match shell {
                Shell::Zsh => {
                    print!("{}", capsule_core::init::zsh::generate());
                }
            }
            Ok(())
        }
    }
}
