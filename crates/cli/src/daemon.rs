//! `capsule daemon` — starts the prompt daemon server.

use std::path::PathBuf;

use anyhow::Context as _;
use capsule_core::{daemon::Server, module::CommandGitProvider};

/// Run the daemon server.
///
/// Binds to `$TMPDIR/capsule.sock`, serves prompt requests, and shuts down
/// on SIGTERM or SIGINT.
///
/// # Errors
///
/// Returns an error if the socket cannot be bound or the runtime fails.
pub fn run() -> anyhow::Result<()> {
    let socket_path = socket_path();
    let home_dir = home_dir()?;
    let git_provider = CommandGitProvider;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        let server = Server::new(socket_path, home_dir, git_provider);

        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        let shutdown = async {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
        };

        server.run(shutdown).await?;
        Ok(())
    })
}

/// Determine the socket path from the environment.
pub fn socket_path() -> PathBuf {
    let tmpdir = std::env::var("TMPDIR")
        .or_else(|_| std::env::var("TMP"))
        .unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(tmpdir).join("capsule.sock")
}

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable not set")
}
