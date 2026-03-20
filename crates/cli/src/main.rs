//! CLI entry point for capsule, a macOS zsh prompt engine.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

mod cli;

use clap::Parser;

use crate::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon => {
            eprintln!("capsule daemon: not yet implemented");
        }
        Command::Connect => {
            eprintln!("capsule connect: not yet implemented");
        }
        Command::Init { shell: _ } => {
            eprintln!("capsule init: not yet implemented");
        }
    }
}
