//! `capsule daemon` — starts the prompt daemon server.

use std::{fs::File, path::PathBuf};

use anyhow::Context as _;
use capsule_core::{daemon::Server, module::CommandGitProvider};

/// Initialize tracing subscriber if `CAPSULE_LOG` is set.
///
/// Writes structured log lines to `$TMPDIR/capsule.log`. The `CAPSULE_LOG`
/// env var controls the filter level (e.g. `debug`, `info`, `capsule_core=debug`).
///
/// # Errors
///
/// Returns an error if the log file cannot be opened.
fn init_tracing() -> anyhow::Result<()> {
    let Ok(filter) = std::env::var("CAPSULE_LOG") else {
        return Ok(());
    };

    let log_path = tmpdir().join("capsule.log");

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&filter))
        .with_writer(file)
        .with_ansi(false)
        .init();

    tracing::info!(path = %log_path.display(), "logging initialized");

    Ok(())
}

/// Run the daemon server.
///
/// Acquires an exclusive file lock (`$TMPDIR/capsule.lock`) to prevent
/// multiple daemons from running simultaneously. If another daemon holds
/// the lock, returns immediately with `Ok(())`.
///
/// Binds to `$TMPDIR/capsule.sock`, serves prompt requests, and shuts down
/// on SIGTERM or SIGINT.
///
/// # Errors
///
/// Returns an error if the lock file cannot be opened, the socket cannot be
/// bound, or the runtime fails.
pub fn run() -> anyhow::Result<()> {
    init_tracing()?;

    let lock_file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path())
        .context("failed to open lock file")?;

    match lock_file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            tracing::info!("another daemon is already running");
            return Ok(());
        }
        Err(std::fs::TryLockError::Error(e)) => {
            return Err(e).context("failed to acquire lock");
        }
    }
    // `lock_file` held until function return — dropping it releases the flock.

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
    tmpdir().join("capsule.sock")
}

fn lock_path() -> PathBuf {
    tmpdir().join("capsule.lock")
}

fn tmpdir() -> PathBuf {
    PathBuf::from(
        std::env::var("TMPDIR")
            .or_else(|_| std::env::var("TMP"))
            .unwrap_or_else(|_| "/tmp".to_owned()),
    )
}

fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable not set")
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    #[test]
    fn test_daemon_flock_prevents_dual_startup() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let lock_path = dir.path().join("capsule.lock");

        let lock_file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        lock_file.try_lock()?;

        // Second attempt should fail with WouldBlock
        let lock_file2 = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;

        match lock_file2.try_lock() {
            Err(std::fs::TryLockError::WouldBlock) => {}
            other => return Err(format!("expected WouldBlock, got {other:?}").into()),
        }

        Ok(())
    }
}
