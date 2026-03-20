//! `capsule connect` — coproc relay between stdin/stdout and the daemon socket.

use std::{path::Path, time::Duration};

use anyhow::Context as _;

use crate::daemon::socket_path;

/// Run the connect relay.
///
/// Auto-starts the daemon if it is not already running, then relays bytes
/// bidirectionally between stdin/stdout and the daemon's Unix socket.
///
/// # Errors
///
/// Returns an error if the daemon cannot be started or the relay fails.
pub fn run() -> anyhow::Result<()> {
    let socket_path = socket_path();

    ensure_daemon(&socket_path)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(relay(&socket_path))
}

/// Ensure the daemon is running. Auto-start if needed.
fn ensure_daemon(socket_path: &Path) -> anyhow::Result<()> {
    // Try connecting to check if daemon is alive
    if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
        return Ok(());
    }

    // Spawn daemon process
    let exe = std::env::current_exe().context("cannot find capsule binary")?;
    std::process::Command::new(&exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to start daemon")?;

    // Wait for socket to become available
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(10));
        if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("daemon failed to start within 1s")
}

/// Bidirectional relay: stdin ↔ socket ↔ stdout.
async fn relay(socket_path: &Path) -> anyhow::Result<()> {
    let stream = tokio::net::UnixStream::connect(socket_path).await?;
    let (mut sock_read, mut sock_write) = stream.into_split();

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    tokio::select! {
        result = tokio::io::copy(&mut stdin, &mut sock_write) => {
            result?;
        }
        result = tokio::io::copy(&mut sock_read, &mut stdout) => {
            result?;
        }
    }

    Ok(())
}
