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
            Some(action @ (DaemonAction::Install | DaemonAction::Uninstall)) => {
                use daemon::{InstallOutcome, ServiceManager as _};

                let home = daemon::home_dir()?;
                let socket_path = daemon::socket_path()?;

                #[cfg(target_os = "macos")]
                let sm = daemon::Launchd::new(&socket_path)?;
                #[cfg(target_os = "linux")]
                let sm = daemon::Systemd::new(&socket_path);
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                anyhow::bail!("service management is not supported on this platform");

                if matches!(action, DaemonAction::Install) {
                    let outcome = sm.install(&home, &socket_path)?;
                    match outcome {
                        InstallOutcome::Installed => {
                            println!("capsule daemon installed and loaded");
                        }
                        InstallOutcome::Restarted => {
                            println!("daemon restarted (binary updated)");
                        }
                        InstallOutcome::AlreadyCurrent => {
                            println!("service is already current, no reload needed");
                        }
                    }
                    Ok(())
                } else {
                    sm.uninstall(&home)
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
