use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

use anyhow::Context as _;

use super::LAUNCHD_SOCKET_NAME;

/// launchd service label.
pub(super) const LAUNCHD_LABEL: &str = "com.github.shuymn.capsule";

/// Abstracts platform-specific service management operations.
///
/// Implementations handle loading, unloading, and restarting a daemon
/// service. `Launchd` dispatches to real `launchctl` commands;
/// `NoopServiceManager` (test-only) succeeds silently.
///
/// When `load` or `restart` returns `Ok`, the daemon is ready to process
/// requests. Implementations that manage a socket-activated daemon
/// block internally until the daemon responds to a protocol handshake.
pub trait ServiceManager {
    /// Load and start the service from the given definition file.
    ///
    /// Returns `Ok(())` once the daemon is ready to process requests.
    fn load(&self, service_file: &Path) -> anyhow::Result<()>;

    /// Stop and unload the service.
    ///
    /// Returns `Ok(true)` if the service was unloaded, `Ok(false)` if it
    /// was not loaded.
    fn unload(&self) -> anyhow::Result<bool>;

    /// Restart a running service.
    ///
    /// Returns `Ok(())` once the daemon is ready to process requests.
    fn restart(&self) -> anyhow::Result<()>;
}

/// macOS launchd service manager.
#[cfg(target_os = "macos")]
pub struct Launchd {
    uid: u32,
    socket_path: PathBuf,
}

#[cfg(target_os = "macos")]
impl Launchd {
    pub fn new(socket_path: &Path) -> anyhow::Result<Self> {
        let output = std::process::Command::new("id")
            .arg("-u")
            .output()
            .context("failed to run `id -u`")?;
        let uid_output = String::from_utf8_lossy(&output.stdout);
        let uid = uid_output
            .trim()
            .parse()
            .with_context(|| format!("failed to parse uid from `id -u`: {uid_output:?}"))?;
        Ok(Self {
            uid,
            socket_path: socket_path.to_path_buf(),
        })
    }

    /// `gui/{uid}/{label}` target used by bootout and kickstart.
    fn service_target(&self) -> String {
        format!("gui/{}/{LAUNCHD_LABEL}", self.uid)
    }
}

#[cfg(target_os = "macos")]
impl ServiceManager for Launchd {
    fn load(&self, service_file: &Path) -> anyhow::Result<()> {
        let status = std::process::Command::new("launchctl")
            .args([
                "bootstrap",
                &format!("gui/{}", self.uid),
                &service_file.to_string_lossy(),
            ])
            .status()
            .context("failed to run launchctl bootstrap")?;
        if !status.success() {
            anyhow::bail!("launchctl bootstrap failed with {status}");
        }
        wait_until_daemon_ready(&self.socket_path, None)
            .context("daemon did not become ready after load")?;
        Ok(())
    }

    fn unload(&self) -> anyhow::Result<bool> {
        let status = std::process::Command::new("launchctl")
            .args(["bootout", &self.service_target()])
            .status()
            .context("failed to run launchctl bootout")?;
        Ok(status.success())
    }

    fn restart(&self) -> anyhow::Result<()> {
        let status = std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &self.service_target()])
            .status()
            .context("failed to run launchctl kickstart")?;
        if !status.success() {
            anyhow::bail!("launchctl kickstart failed with {status}");
        }
        let expected = crate::build_id::compute();
        wait_until_daemon_ready(&self.socket_path, expected.as_ref())
            .context("daemon did not become ready after restart")?;
        Ok(())
    }
}

/// Generate the launchd plist XML for the capsule daemon.
///
/// Uses socket activation: launchd creates the socket and launches
/// the daemon on first connection. The daemon retrieves the socket fd
/// via `launch_activate_socket`.
pub(super) fn generate_plist(capsule_bin: &Path, socket_path: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>daemon</string>
    </array>
    <key>Sockets</key>
    <dict>
        <key>{LAUNCHD_SOCKET_NAME}</key>
        <dict>
            <key>SockPathName</key>
            <string>{}</string>
            <key>SockPathMode</key>
            <integer>448</integer>
        </dict>
    </dict>
</dict>
</plist>
"#,
        capsule_bin.display(),
        socket_path.display(),
    )
}

/// Compute the plist path for a given home directory.
pub(super) fn plist_path_for(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

/// Install the launchd plist and load the daemon service.
///
/// Idempotent: if the plist already exists with identical content and the
/// running daemon's build ID matches the current binary, no reload occurs.
/// If the plist content differs, the service is reloaded. If the plist is
/// current but the binary has been updated, the daemon is restarted via
/// `launchctl kickstart -k`.
///
/// # Errors
///
/// Returns an error if the plist cannot be written or the service manager
/// operation fails.
pub fn install(sm: &impl ServiceManager, home: &Path, socket_path: &Path) -> anyhow::Result<()> {
    let capsule_bin = std::env::current_exe().context("cannot find capsule binary")?;
    let plist_content = generate_plist(&capsule_bin, socket_path);
    let plist = plist_path_for(home);

    if let Ok(existing) = std::fs::read_to_string(&plist) {
        if existing == plist_content {
            if daemon_needs_restart(socket_path) {
                sm.restart()?;
                println!("daemon restarted (binary updated)");
            } else {
                println!("plist is already current, no reload needed");
            }
            return Ok(());
        }
        let _ = sm.unload();
    }

    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    std::fs::write(&plist, &plist_content)
        .with_context(|| format!("failed to write {}", plist.display()))?;

    sm.load(&plist)?;
    println!("capsule daemon installed and loaded");

    Ok(())
}

/// Uninstall the launchd service and remove the plist.
///
/// # Errors
///
/// Returns an error if the service manager operation fails or the plist
/// cannot be removed.
pub fn uninstall(sm: &impl ServiceManager, home: &Path) -> anyhow::Result<()> {
    let plist = plist_path_for(home);

    if !sm.unload()? {
        eprintln!("warning: service unload exited with non-zero status");
    }

    match std::fs::remove_file(&plist) {
        Ok(()) => println!("capsule daemon uninstalled"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("plist not found, nothing to remove");
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to remove {}", plist.display()));
        }
    }

    Ok(())
}

/// Check if a running daemon needs to be restarted due to a binary update.
///
/// Returns `true` if the daemon is running and its build ID differs from
/// the current binary. Returns `false` if build IDs match, the daemon is
/// unreachable, or the local build ID cannot be computed.
pub(super) fn daemon_needs_restart(socket_path: &Path) -> bool {
    matches!(
        crate::connect::negotiate_build_id(socket_path),
        Ok(ref n) if n.needs_daemon_restart(),
    )
}

/// Poll until the daemon responds to a Hello/HelloAck handshake.
///
/// Unlike a simple `UnixStream::connect` check, this verifies the daemon
/// is actually processing connections. With launchd socket activation the
/// socket is always connectable (launchd manages it), so a connect-only
/// check returns immediately even before the daemon process starts.
///
/// If `expected_build_id` is `Some`, the `HelloAck` must contain a
/// matching build ID (used after restart to confirm the *new* daemon
/// is responding, not the old one being torn down). If `None`, any
/// `HelloAck` suffices (used after fresh load).
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
