//! `capsule daemon` — starts the prompt daemon server.

mod service;
mod status;

use std::{fs::File, io::Write as _, path::PathBuf};

use anyhow::Context as _;
use capsule_core::{
    config,
    daemon::{
        ConfigSource, Server,
        listener::{ListenerMode, ListenerSource, acquire_listener},
    },
    module::CommandGitProvider,
};
#[cfg(target_os = "macos")]
pub use service::Launchd;
#[cfg(target_os = "linux")]
pub use service::Systemd;
#[cfg(target_os = "macos")]
use service::launchd::LAUNCHD_SOCKET_NAME;
pub use service::{InstallOutcome, ServiceManager, reinstall_service_if_present};
pub use status::status;

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
/// In standalone (bind) mode, acquires an exclusive file lock
/// (`~/.capsule/capsule.lock`) to prevent multiple daemons. If another
/// daemon holds the lock, returns immediately with `Ok(())`.
///
/// In socket-activation mode, the flock is skipped (the service manager
/// guarantees single instance).
///
/// # Errors
///
/// Returns an error if the lock file cannot be opened, the listener cannot
/// be acquired, or the runtime fails.
pub fn run() -> anyhow::Result<()> {
    init_tracing()?;

    // Ensure ~/.capsule/ exists (for socket and lock files).
    let dir = capsule_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    // Try platform socket activation first.
    #[cfg(target_os = "macos")]
    {
        let source = ListenerSource::Launchd(LAUNCHD_SOCKET_NAME.to_owned());
        if let Ok(listener) = acquire_listener(&source) {
            return run_server(listener, source.mode());
        }
    }
    #[cfg(target_os = "linux")]
    {
        let source = ListenerSource::Systemd;
        if let Ok(listener) = acquire_listener(&source) {
            return run_server(listener, source.mode());
        }
    }

    // Standalone mode: flock before bind.
    let Some(_lock_file) = acquire_flock()? else {
        return Ok(()); // another daemon is running
    };

    let source = ListenerSource::Bind(socket_path()?);
    let listener = acquire_listener(&source).context("failed to bind socket")?;

    run_server(listener, source.mode())
}

/// Start the server with an already-acquired listener.
fn run_server(
    listener: std::os::unix::net::UnixListener,
    mode: ListenerMode,
) -> anyhow::Result<()> {
    let home_dir = home_dir()?;
    let git_provider = CommandGitProvider;
    let build_id = crate::build_id::compute();
    let config_path = config::resolve_config_path();
    let cfg = std::sync::Arc::new(
        config_path
            .as_deref()
            .map_or_else(config::Config::default, config::load_config),
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        let server = Server::new(
            home_dir,
            git_provider,
            build_id,
            mode,
            ConfigSource::new(cfg, config_path),
        );

        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        let shutdown = async {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
        };

        server.run(listener, shutdown).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// flock
// ---------------------------------------------------------------------------

/// Acquire the flock and write PID. Returns `Some(file)` if acquired,
/// `None` if another daemon already holds the lock.
fn acquire_flock() -> anyhow::Result<Option<File>> {
    let mut lock_file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path()?)
        .context("failed to open lock file")?;

    match lock_file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            tracing::info!("another daemon is already running");
            return Ok(None);
        }
        Err(std::fs::TryLockError::Error(e)) => {
            return Err(e).context("failed to acquire lock");
        }
    }

    // Write PID so `capsule connect` can send SIGTERM on build_id mismatch.
    lock_file
        .set_len(0)
        .context("failed to truncate lock file")?;
    write!(lock_file, "{}", std::process::id()).context("failed to write PID to lock file")?;

    Ok(Some(lock_file))
}

/// Determine the socket path.
///
/// # Errors
///
/// Returns an error if the base directory cannot be determined.
pub fn socket_path() -> anyhow::Result<PathBuf> {
    Ok(capsule_dir()?.join("capsule.sock"))
}

/// Path to the daemon lock file.
///
/// # Errors
///
/// Returns an error if the base directory cannot be determined.
pub fn lock_path() -> anyhow::Result<PathBuf> {
    Ok(capsule_dir()?.join("capsule.lock"))
}

/// Base directory for capsule runtime files.
///
/// # Errors
///
/// Returns an error if `$HOME` is not set.
fn capsule_dir() -> anyhow::Result<PathBuf> {
    Ok(home_dir()?.join(".capsule"))
}

/// Log file directory (`$TMPDIR`).
///
/// macOS always sets `$TMPDIR` to a per-user temporary directory.
fn tmpdir() -> PathBuf {
    PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_owned()))
}

pub fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable not set")
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{BufRead as _, Write as _},
        path::Path,
        time::Duration,
    };

    use capsule_protocol::{BuildId, HelloAck, Message, PROTOCOL_VERSION};

    use super::service::{
        InstallOutcome, ServiceManager, daemon_needs_restart, wait_until_daemon_ready,
    };

    struct NoopServiceManager;

    impl ServiceManager for NoopServiceManager {
        fn install(&self, _home: &Path, _socket_path: &Path) -> anyhow::Result<InstallOutcome> {
            Ok(InstallOutcome::Installed)
        }

        fn uninstall(&self, _home: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        fn restart(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Start a mock daemon that responds to Hello with a `HelloAck`
    /// containing the specified build ID. The listener uses non-blocking
    /// accept with a timeout so the thread does not hang if no client
    /// connects.
    fn start_mock_daemon(
        socket_path: &Path,
        respond_build_id: Option<BuildId>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let listener = std::os::unix::net::UnixListener::bind(socket_path)?;
        listener.set_nonblocking(true)?;
        std::thread::spawn(move || {
            // Poll accept for up to 5 seconds.
            let stream = {
                let mut result = None;
                for _ in 0..500 {
                    match listener.accept() {
                        Ok((s, _)) => {
                            result = Some(s);
                            break;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => return,
                    }
                }
                let Some(stream) = result else { return };
                stream
            };

            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

            // Read Hello (newline-delimited)
            {
                let mut reader = std::io::BufReader::new(&stream);
                let mut buf = Vec::new();
                let _ = reader.read_until(b'\n', &mut buf);
            }

            // Send `HelloAck`
            let ack = Message::HelloAck(HelloAck {
                version: PROTOCOL_VERSION,
                build_id: respond_build_id,
                env_var_names: vec![],
            });
            let mut wire = ack.to_wire();
            wire.push(b'\n');
            let _ = (&stream).write_all(&wire);
        });
        Ok(())
    }

    #[test]
    fn test_daemon_flock_blocks_second() -> Result<(), Box<dyn std::error::Error>> {
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

    #[test]
    fn test_noop_install() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let socket = home.path().join("capsule.sock");
        let outcome = NoopServiceManager.install(home.path(), &socket)?;
        assert_eq!(outcome, InstallOutcome::Installed);
        Ok(())
    }

    #[test]
    fn test_noop_uninstall() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        NoopServiceManager.uninstall(home.path())?;
        Ok(())
    }

    #[test]
    fn test_install_build_id_match() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("test.sock");

        // Mock daemon echoes back the same build_id as the current binary.
        let my_id = crate::build_id::compute();
        start_mock_daemon(&socket, my_id)?;

        assert!(
            !daemon_needs_restart(&socket),
            "should not restart when build IDs match"
        );

        Ok(())
    }

    #[test]
    fn test_install_build_id_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("test.sock");

        // Mock daemon returns a different build_id.
        start_mock_daemon(&socket, Some(BuildId::new("different:12345".to_owned())))?;

        assert!(
            daemon_needs_restart(&socket),
            "should restart when build IDs differ"
        );

        Ok(())
    }

    #[test]
    fn test_install_daemon_unreachable() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("nonexistent.sock");

        assert!(
            !daemon_needs_restart(&socket),
            "should not restart when daemon is unreachable"
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // wait_until_daemon_ready tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_wait_until_daemon_ready_hello() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("test.sock");

        start_mock_daemon(&socket, None)?;
        std::thread::sleep(Duration::from_millis(50));

        wait_until_daemon_ready(&socket, None)?;
        Ok(())
    }

    #[test]
    fn test_wait_until_daemon_ready_build_id() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("test.sock");

        let my_id = crate::build_id::compute();
        start_mock_daemon(&socket, my_id.clone())?;
        std::thread::sleep(Duration::from_millis(50));

        wait_until_daemon_ready(&socket, my_id.as_ref())?;
        Ok(())
    }

    #[test]
    fn test_wait_until_daemon_ready_no_listener() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("nonexistent.sock");

        let result = wait_until_daemon_ready(&socket, None);
        assert!(result.is_err(), "should fail on nonexistent socket");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // reinstall_service_if_present tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_reinstall_returns_none_without_service() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let socket = home.path().join("capsule.sock");

        let result = super::service::reinstall_service_if_present(home.path(), &socket)?;
        assert!(
            result.is_none(),
            "should return None when no service is installed"
        );
        Ok(())
    }
}
