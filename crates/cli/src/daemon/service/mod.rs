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

/// Outcome of a [`ServiceManager::install`] operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// A new service definition was written and the daemon was loaded.
    Installed,
    /// The service definition was already current; daemon was restarted
    /// because the binary was updated.
    Restarted,
    /// Everything is already up-to-date; no action was taken.
    AlreadyCurrent,
}

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
    /// Returns the [`InstallOutcome`] describing what action was taken.
    ///
    /// # Errors
    ///
    /// Returns an error if the service definition cannot be written or the
    /// service manager operation fails.
    fn install(&self, home: &Path, socket_path: &Path) -> anyhow::Result<InstallOutcome>;

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

/// Reinstall the service definition if one is already present.
///
/// Checks whether a platform service definition (launchd plist or systemd
/// unit files) exists. If so, runs the full [`ServiceManager::install`] flow
/// — which regenerates the definition, reloads it, and restarts the daemon
/// as needed — then returns the [`InstallOutcome`].
///
/// Returns `Ok(None)` if no service definition is installed (i.e. the daemon
/// runs in standalone mode), the platform is unsupported, or the service
/// definition is managed declaratively by Nix.
///
/// # Errors
///
/// Returns an error if the service reinstall fails.
pub fn reinstall_service_if_present(
    home: &Path,
    socket_path: &Path,
) -> anyhow::Result<Option<InstallOutcome>> {
    if nix_managed_service_definition(home).is_some() {
        return Ok(None);
    }

    #[cfg(target_os = "macos")]
    {
        let plist = launchd::plist_path_for(home);
        if plist.exists() {
            let sm = Launchd::new(socket_path)?;
            return Ok(Some(sm.install(home, socket_path)?));
        }
    }

    #[cfg(target_os = "linux")]
    {
        let service_file = systemd::service_file_path(home);
        if service_file.exists() {
            let sm = Systemd::new(socket_path);
            return Ok(Some(sm.install(home, socket_path)?));
        }
    }

    let _ = (home, socket_path);
    Ok(None)
}

/// Return the Nix-managed daemon definition path, if one is present.
pub fn nix_managed_service_definition(home: &Path) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = launchd::nix_managed_plist_path(home) {
            return Some(path);
        }
    }

    #[cfg(target_os = "linux")]
    if let Some(path) = systemd::nix_managed_service_file_path(home) {
        return Some(path);
    }

    let _ = home;
    None
}

const NIX_MANAGED_MARKER: &str = "CAPSULE_NIX_MANAGED";

fn is_nix_managed_definition(path: &Path) -> bool {
    definition_contains_nix_marker(path)
        || is_nix_store_path(path)
        || symlink_target_is_nix_store_path(path)
        || std::fs::canonicalize(path).is_ok_and(|resolved| is_nix_store_path(&resolved))
}

fn definition_contains_nix_marker(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|content| content.contains(NIX_MANAGED_MARKER))
}

fn symlink_target_is_nix_store_path(path: &Path) -> bool {
    std::fs::read_link(path).is_ok_and(|target| {
        let absolute_target = if target.is_absolute() {
            target
        } else if let Some(parent) = path.parent() {
            parent.join(target)
        } else {
            target
        };
        is_nix_store_path(&absolute_target)
    })
}

fn is_nix_store_path(path: &Path) -> bool {
    path.starts_with("/nix/store")
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
pub fn wait_until_daemon_ready(
    socket_path: &Path,
    expected_build_id: Option<&capsule_protocol::BuildId>,
) -> anyhow::Result<()> {
    use capsule_protocol::{Hello, Message};

    const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(200);
    const MAX_ATTEMPTS: u32 = 25;

    let hello = Message::Hello(Hello {
        build_id: crate::build_id::compute(),
    });

    for _ in 0..MAX_ATTEMPTS {
        if let Ok(Message::HelloAck(ack)) =
            crate::connect::sync_request_with_timeout(socket_path, &hello, ATTEMPT_TIMEOUT)
        {
            let id_ok = match expected_build_id {
                Some(expected) => ack.build_id.as_ref() == Some(expected),
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
pub fn wait_until_daemon_ready(
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{NIX_MANAGED_MARKER, is_nix_managed_definition};

    #[cfg(unix)]
    #[test]
    fn test_nix_managed_definition_detects_nix_store_symlink()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let definition = dir.path().join("capsule.service");

        std::os::unix::fs::symlink("/nix/store/example-capsule.service", &definition)?;

        assert!(
            is_nix_managed_definition(&definition),
            "definition symlinked into /nix/store should be treated as Nix-managed"
        );
        Ok(())
    }

    #[test]
    fn test_nix_managed_definition_detects_marker() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let definition = dir.path().join("com.github.shuymn.capsule.plist");

        std::fs::write(
            &definition,
            format!("<key>{NIX_MANAGED_MARKER}</key>\n<string>1</string>\n"),
        )?;

        assert!(
            is_nix_managed_definition(&definition),
            "definition containing the Nix marker should be treated as Nix-managed"
        );
        Ok(())
    }

    #[test]
    fn test_nix_managed_definition_ignores_regular_home_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let definition = dir.path().join("capsule.service");

        std::fs::write(
            &definition,
            "[Service]\nExecStart=/usr/local/bin/capsule daemon\n",
        )?;

        assert!(
            !is_nix_managed_definition(&definition),
            "regular files outside /nix/store should remain imperatively managed"
        );
        Ok(())
    }

    #[test]
    fn test_nix_managed_definition_ignores_missing_file() {
        assert!(
            !is_nix_managed_definition(Path::new("/tmp/capsule-missing.service")),
            "missing definitions should not be treated as Nix-managed"
        );
    }
}
