use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::thread;
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
/// When `load` or `restart` returns `Ok`, the service is ready to accept
/// connections. Implementations that manage a socket-activated daemon
/// block internally until the socket is connectable.
pub trait ServiceManager {
    /// Load and start the service from the given definition file.
    ///
    /// Returns `Ok(())` once the service is ready to accept connections.
    fn load(&self, service_file: &Path) -> anyhow::Result<()>;

    /// Stop and unload the service.
    ///
    /// Returns `Ok(true)` if the service was unloaded, `Ok(false)` if it
    /// was not loaded.
    fn unload(&self) -> anyhow::Result<bool>;

    /// Restart a running service.
    ///
    /// Returns `Ok(())` once the service is ready to accept connections.
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
        wait_until_socket_connectable(&self.socket_path)
            .context("daemon socket did not become ready after load")?;
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
        wait_until_socket_connectable(&self.socket_path)
            .context("daemon socket did not become ready after restart")?;
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

/// Poll until `socket_path` accepts a Unix stream connection or a timeout elapses.
///
/// Used after launchd restart/load so `capsule connect` in existing shells can
/// reconnect without racing a still-starting daemon.
#[cfg(unix)]
fn wait_until_socket_connectable(socket_path: &Path) -> anyhow::Result<()> {
    use std::os::unix::net::UnixStream;

    const INTERVAL: Duration = Duration::from_millis(10);
    const MAX_ATTEMPTS: u32 = 500;

    for _ in 0..MAX_ATTEMPTS {
        if UnixStream::connect(socket_path).is_ok() {
            return Ok(());
        }
        thread::sleep(INTERVAL);
    }

    anyhow::bail!(
        "timed out after {} ms waiting for {}",
        INTERVAL.as_millis() * u128::from(MAX_ATTEMPTS),
        socket_path.display()
    )
}

#[cfg(not(unix))]
fn wait_until_socket_connectable(_socket_path: &Path) -> anyhow::Result<()> {
    Ok(())
}
