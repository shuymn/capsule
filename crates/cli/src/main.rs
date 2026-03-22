//! CLI entry point for capsule, a macOS zsh prompt engine.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

mod build_id;
mod cli;
mod connect;
mod daemon;
mod preset;

use clap::Parser;

use crate::cli::{Cli, Command, DaemonAction, Shell};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Daemon { action } => match action {
            None => daemon::run(),
            Some(DaemonAction::Status { json }) => daemon::status(json),
            Some(DaemonAction::Install | DaemonAction::Uninstall) => {
                #[cfg(not(target_os = "macos"))]
                anyhow::bail!("capsule daemon install/uninstall requires macOS (launchd)");
                #[cfg(target_os = "macos")]
                {
                    let home = daemon::home_dir()?;
                    let socket_path = daemon::socket_path()?;
                    let sm = daemon::Launchd::new(&socket_path)?;
                    match action {
                        Some(DaemonAction::Install) => daemon::install(&sm, &home, &socket_path),
                        Some(DaemonAction::Uninstall) => daemon::uninstall(&sm, &home),
                        _ => Ok(()),
                    }
                }
            }
        },
        Command::Connect => connect::run(),
        Command::Init { shell } => {
            match shell {
                Shell::Zsh => {
                    print!("{}", capsule_core::init::zsh::generate());
                }
            }
            Ok(())
        }
        Command::Preset => preset::run(),
    }
}
