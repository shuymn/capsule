//! Platform-specific service management abstraction.

use std::path::Path;
#[cfg(unix)]
use std::time::Duration;

#[cfg(any(target_os = "macos", test))]
pub(super) mod launchd;
#[cfg(target_os = "linux")]
pub(super) mod systemd;

#[cfg(target_os = "macos")]
pub use launchd::Launchd;
#[cfg(target_os = "linux")]
pub use systemd::Systemd;

/// Abstracts platform-specific service management operations.
///
/// Implementations handle installing, uninstalling, and restarting a daemon
/// service. `Launchd` dispatches to `launchctl` on macOS; `Systemd`
/// dispatches to `systemctl` on Linux.
///
/// When [`install`](ServiceManager::install) or
/// [`restart`](ServiceManager::restart) returns `Ok`, the daemon is ready to
/// process requests.
pub trait ServiceManager {
    /// Generate and install the service definition, then start the daemon.
    ///
    /// Idempotent: if the service definition is already current and the
    /// daemon's build ID matches the current binary, no reload occurs.
    ///
    /// Returns `Ok(())` once the daemon is ready to process requests.
    ///
    /// # Errors
    ///
    /// Returns an error if the service definition cannot be written or the
    /// service manager operation fails.
    fn install(&self, home: &Path, socket_path: &Path) -> anyhow::Result<()>;

    /// Stop the daemon and remove the service definition.
    ///
    /// # Errors
    ///
    /// Returns an error if the service cannot be stopped or files cannot be
    /// removed.
    fn uninstall(&self, home: &Path) -> anyhow::Result<()>;

    /// Restart a running service.
    ///
    /// Returns `Ok(())` once the daemon is ready to process requests.
    ///
    /// # Errors
    ///
    /// Returns an error if the service manager operation fails or the daemon
    /// does not become ready.
    fn restart(&self) -> anyhow::Result<()>;
}

/// Check if a running daemon needs to be restarted due to a binary update.
///
/// Returns `true` if the daemon is running and its build ID differs from the
/// current binary. Returns `false` if build IDs match, the daemon is
/// unreachable, or the local build ID cannot be computed.
pub(super) fn daemon_needs_restart(socket_path: &Path) -> bool {
    matches!(
        crate::connect::negotiate_build_id(socket_path),
        Ok(ref n) if n.needs_daemon_restart(),
    )
}

/// Poll until the daemon responds to a `Hello`/`HelloAck` handshake.
///
/// Unlike a simple `UnixStream::connect` check, this verifies the daemon is
/// actually processing connections. With socket activation the socket is
/// always connectable (the service manager owns it), so a connect-only check
/// returns immediately even before the daemon process starts.
///
/// If `expected_build_id` is `Some`, the `HelloAck` must contain a matching
/// build ID (used after restart to confirm the *new* daemon is responding, not
/// the old one being torn down). If `None`, any `HelloAck` suffices (used
/// after fresh load).
///
/// # Errors
///
/// Returns an error if the daemon does not respond within the timeout.
#[cfg(unix)]
pub(super) fn wait_until_daemon_ready(
    socket_path: &Path,
    expected_build_id: Option<&capsule_protocol::BuildId>,
) -> anyhow::Result<()> {
    use capsule_protocol::{Hello, Message, PROTOCOL_VERSION};

    const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(200);
    const MAX_ATTEMPTS: u32 = 25;

    let hello = Message::Hello(Hello {
        version: PROTOCOL_VERSION,
        build_id: crate::build_id::compute(),
    });

    for _ in 0..MAX_ATTEMPTS {
        if let Ok(Message::HelloAck(ack)) =
            crate::connect::sync_request(socket_path, &hello, ATTEMPT_TIMEOUT)
        {
            let id_ok = match expected_build_id {
                Some(expected) => ack.build_id.as_ref().is_none_or(|id| id == expected),
                None => true,
            };
            if id_ok {
                return Ok(());
            }
            // Old daemon still responding; wait before retrying.
            std::thread::sleep(ATTEMPT_TIMEOUT);
        }
    }

    anyhow::bail!(
        "daemon did not become ready within {} ms ({})",
        ATTEMPT_TIMEOUT.as_millis() * u128::from(MAX_ATTEMPTS),
        socket_path.display()
    )
}

#[cfg(not(unix))]
pub(super) fn wait_until_daemon_ready(
    _socket_path: &Path,
    _expected_build_id: Option<&capsule_protocol::BuildId>,
) -> anyhow::Result<()> {
    Ok(())
}

/// Collect environment variables that must be forwarded to the daemon process.
///
/// Returns name-value pairs for variables that affect config resolution so that
/// socket-activated daemons behave identically to interactive shell sessions.
pub(super) fn collect_forwarded_env() -> Vec<(&'static str, String)> {
    let mut vars = Vec::new();
    if let Ok(val) = std::env::var("XDG_CONFIG_HOME") {
        vars.push(("XDG_CONFIG_HOME", val));
    }
    vars
}
