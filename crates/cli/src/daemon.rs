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
pub use service::{install, uninstall};
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

/// launchd socket name matching the plist `SockServiceName`.
const LAUNCHD_SOCKET_NAME: &str = "Listeners";

/// Run the daemon server.
///
/// In standalone (bind) mode, acquires an exclusive file lock
/// (`~/.capsule/capsule.lock`) to prevent multiple daemons. If another
/// daemon holds the lock, returns immediately with `Ok(())`.
///
/// In launchd mode, flock is skipped (launchd guarantees single instance).
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

    // Try launchd socket activation first.
    let launchd_source = ListenerSource::Launchd(LAUNCHD_SOCKET_NAME.to_owned());
    if let Ok(listener) = acquire_listener(&launchd_source) {
        return run_server(listener, launchd_source.mode());
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
/// Uses `$CAPSULE_SOCK_DIR` (for testing) or `~/.capsule/`.
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
/// Uses `$CAPSULE_SOCK_DIR` if set (for testing), otherwise `~/.capsule/`.
///
/// # Errors
///
/// Returns an error if neither `$CAPSULE_SOCK_DIR` nor `$HOME` is set.
fn capsule_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("CAPSULE_SOCK_DIR") {
        return Ok(PathBuf::from(dir));
    }
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
        path::{Path, PathBuf},
        time::Duration,
    };

    use capsule_protocol::{BuildId, HelloAck, Message, PROTOCOL_VERSION};

    use super::{
        service::{
            LAUNCHD_LABEL, ServiceManager, daemon_needs_restart, generate_plist, plist_path_for,
        },
        *,
    };

    struct NoopServiceManager;

    impl ServiceManager for NoopServiceManager {
        fn load(&self, _service_file: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        fn unload(&self) -> anyhow::Result<bool> {
            Ok(true)
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

    #[test]
    fn test_generate_plist_contains_required_keys() {
        let bin = PathBuf::from("/usr/local/bin/capsule");
        let sock = PathBuf::from("/Users/test/.capsule/capsule.sock");
        let plist = generate_plist(&bin, &sock);

        assert!(plist.contains(LAUNCHD_LABEL), "plist should contain label");
        assert!(
            plist.contains("/usr/local/bin/capsule"),
            "plist should contain binary path"
        );
        assert!(
            plist.contains("daemon"),
            "plist should contain 'daemon' argument"
        );
        assert!(
            plist.contains(LAUNCHD_SOCKET_NAME),
            "plist should contain socket name"
        );
        assert!(
            plist.contains("/Users/test/.capsule/capsule.sock"),
            "plist should contain socket path"
        );
        assert!(
            plist.contains("SockPathMode"),
            "plist should set socket permissions"
        );
    }

    #[test]
    fn test_generate_plist_no_inetd_compatibility() {
        let bin = PathBuf::from("/usr/local/bin/capsule");
        let sock = PathBuf::from("/Users/test/.capsule/capsule.sock");
        let plist = generate_plist(&bin, &sock);

        assert!(
            !plist.contains("inetdCompatibility"),
            "plist should not use inetdCompatibility"
        );
    }

    #[test]
    fn test_generate_plist_valid_xml() {
        let bin = PathBuf::from("/usr/local/bin/capsule");
        let sock = PathBuf::from("/Users/test/.capsule/capsule.sock");
        let plist = generate_plist(&bin, &sock);

        assert!(
            plist.starts_with("<?xml"),
            "plist should start with XML declaration"
        );
        assert!(plist.contains("</plist>"), "plist should be well-formed");
    }

    #[test]
    fn test_install_build_id_match_skips_restart() -> Result<(), Box<dyn std::error::Error>> {
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
    fn test_install_build_id_mismatch_triggers_restart() -> Result<(), Box<dyn std::error::Error>> {
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
    fn test_install_daemon_unreachable_skips_restart() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("nonexistent.sock");

        assert!(
            !daemon_needs_restart(&socket),
            "should not restart when daemon is unreachable"
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // ServiceManager + install/uninstall flow tests (Noop variant)
    // -----------------------------------------------------------------------

    #[test]
    fn test_install_fresh_creates_plist_noop() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;

        install(&NoopServiceManager, home.path())?;

        let plist = plist_path_for(home.path());
        assert!(plist.is_file(), "plist should be created");
        let content = std::fs::read_to_string(&plist)?;
        assert!(
            content.contains(LAUNCHD_LABEL),
            "plist should contain label"
        );
        Ok(())
    }

    #[test]
    fn test_install_unchanged_plist_skips_noop() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;

        // First install creates the plist.
        install(&NoopServiceManager, home.path())?;
        let plist = plist_path_for(home.path());
        let content_before = std::fs::read_to_string(&plist)?;

        // Second install with same binary — no daemon running → skip.
        install(&NoopServiceManager, home.path())?;
        let content_after = std::fs::read_to_string(&plist)?;

        assert_eq!(
            content_before, content_after,
            "plist should not change on repeated install"
        );
        Ok(())
    }

    #[test]
    fn test_install_unchanged_plist_build_id_mismatch_noop()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;

        // First install.
        install(&NoopServiceManager, home.path())?;

        // Start mock daemon returning a different build_id on the canonical socket.
        let capsule_dir = home.path().join(".capsule");
        std::fs::create_dir_all(&capsule_dir)?;
        let socket = capsule_dir.join("capsule.sock");
        start_mock_daemon(&socket, Some(BuildId::new("different:99999".to_owned())))?;
        std::thread::sleep(Duration::from_millis(50));

        // Second install detects mismatch → kickstart via Noop succeeds.
        install(&NoopServiceManager, home.path())?;

        // Plist content should be unchanged (kickstart, not rewrite).
        let plist = plist_path_for(home.path());
        assert!(plist.is_file(), "plist should still exist");
        Ok(())
    }

    #[test]
    fn test_install_changed_plist_reloads_noop() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let plist = plist_path_for(home.path());

        // Create an old plist with different content.
        if let Some(parent) = plist.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&plist, "old content")?;

        // Install should replace it.
        install(&NoopServiceManager, home.path())?;

        let content = std::fs::read_to_string(&plist)?;
        assert_ne!(content, "old content", "plist should be updated");
        assert!(
            content.contains(LAUNCHD_LABEL),
            "plist should contain label"
        );
        Ok(())
    }

    #[test]
    fn test_uninstall_removes_plist_noop() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;

        // Install first.
        install(&NoopServiceManager, home.path())?;
        let plist = plist_path_for(home.path());
        assert!(plist.is_file(), "plist should exist before uninstall");

        // Uninstall removes plist.
        uninstall(&NoopServiceManager, home.path())?;
        assert!(!plist.exists(), "plist should be removed after uninstall");
        Ok(())
    }

    #[test]
    fn test_uninstall_missing_plist_noop() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;

        // Uninstall when plist doesn't exist — should succeed gracefully.
        uninstall(&NoopServiceManager, home.path())?;
        Ok(())
    }
}
