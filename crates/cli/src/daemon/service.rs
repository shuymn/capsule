use std::path::{Path, PathBuf};

use anyhow::Context as _;

use super::LAUNCHD_SOCKET_NAME;

/// launchd service label.
pub(super) const LAUNCHD_LABEL: &str = "com.github.shuymn.capsule";

/// Abstracts platform-specific service management operations.
///
/// Implementations handle loading, unloading, and restarting a daemon
/// service. `Launchd` dispatches to real `launchctl` commands;
/// `NoopServiceManager` (test-only) succeeds silently.
pub trait ServiceManager {
    /// Load and start the service from the given definition file.
    fn load(&self, service_file: &Path) -> anyhow::Result<()>;

    /// Stop and unload the service.
    ///
    /// Returns `Ok(true)` if the service was unloaded, `Ok(false)` if it
    /// was not loaded.
    fn unload(&self) -> anyhow::Result<bool>;

    /// Restart a running service.
    fn restart(&self) -> anyhow::Result<()>;
}

/// macOS launchd service manager.
#[cfg(target_os = "macos")]
pub struct Launchd {
    uid: u32,
}

#[cfg(target_os = "macos")]
impl Launchd {
    pub fn new() -> anyhow::Result<Self> {
        let output = std::process::Command::new("id")
            .arg("-u")
            .output()
            .context("failed to run `id -u`")?;
        let uid_output = String::from_utf8_lossy(&output.stdout);
        let uid = uid_output
            .trim()
            .parse()
            .with_context(|| format!("failed to parse uid from `id -u`: {uid_output:?}"))?;
        Ok(Self { uid })
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
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("launchctl bootstrap failed with {status}");
        }
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
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("launchctl kickstart failed with {status}");
        }
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
pub fn install(sm: &impl ServiceManager, home: &Path) -> anyhow::Result<()> {
    let capsule_bin = std::env::current_exe().context("cannot find capsule binary")?;
    let canonical_socket = home.join(".capsule/capsule.sock");
    let plist_content = generate_plist(&capsule_bin, &canonical_socket);
    let plist = plist_path_for(home);

    if let Ok(existing) = std::fs::read_to_string(&plist) {
        if existing == plist_content {
            if daemon_needs_restart(&canonical_socket) {
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
        Ok(crate::connect::NegotiationResult {
            build_id_ok: false,
            ..
        })
    )
}
