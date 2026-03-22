//! macOS launchd service manager.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use anyhow::Context as _;

#[cfg(target_os = "macos")]
use super::ServiceManager;

/// launchd service label.
const LAUNCHD_LABEL: &str = "com.github.shuymn.capsule";

/// launchd socket name matching the plist `SockServiceName`.
pub const LAUNCHD_SOCKET_NAME: &str = "Listeners";

/// macOS launchd service manager.
#[cfg(target_os = "macos")]
pub struct Launchd {
    uid: u32,
    socket_path: PathBuf,
}

#[cfg(target_os = "macos")]
impl Launchd {
    /// Create a new [`Launchd`] service manager.
    ///
    /// # Errors
    ///
    /// Returns an error if the current user ID cannot be determined.
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

    /// `gui/{uid}/{label}` target used by `bootout` and `kickstart`.
    fn service_target(&self) -> String {
        format!("gui/{}/{LAUNCHD_LABEL}", self.uid)
    }

    /// Bootstrap the service from the given plist file.
    fn load(&self, plist: &Path) -> anyhow::Result<()> {
        let status = std::process::Command::new("launchctl")
            .args([
                "bootstrap",
                &format!("gui/{}", self.uid),
                &plist.to_string_lossy(),
            ])
            .status()
            .context("failed to run launchctl bootstrap")?;
        if !status.success() {
            anyhow::bail!("launchctl bootstrap failed with {status}");
        }
        super::wait_until_daemon_ready(&self.socket_path, None)
            .context("daemon did not become ready after load")?;
        Ok(())
    }

    /// Bootout (unload) the service.
    ///
    /// Returns `true` if the service was unloaded, `false` if it was not loaded.
    fn unload(&self) -> anyhow::Result<bool> {
        let status = std::process::Command::new("launchctl")
            .args(["bootout", &self.service_target()])
            .status()
            .context("failed to run launchctl bootout")?;
        Ok(status.success())
    }
}

#[cfg(target_os = "macos")]
impl ServiceManager for Launchd {
    fn install(&self, home: &Path, socket_path: &Path) -> anyhow::Result<()> {
        let capsule_bin = std::env::current_exe().context("cannot find capsule binary")?;
        let forwarded_env = super::collect_forwarded_env();
        let plist_content = generate_plist(&capsule_bin, socket_path, &forwarded_env);
        let plist = plist_path_for(home);

        if let Ok(existing) = std::fs::read_to_string(&plist) {
            if existing == plist_content {
                if super::daemon_needs_restart(socket_path) {
                    self.restart()?;
                    println!("daemon restarted (binary updated)");
                } else {
                    println!("plist is already current, no reload needed");
                }
                return Ok(());
            }
            let _ = self.unload();
        }

        if let Some(parent) = plist.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        std::fs::write(&plist, &plist_content)
            .with_context(|| format!("failed to write {}", plist.display()))?;

        self.load(&plist)?;
        println!("capsule daemon installed and loaded");

        Ok(())
    }

    fn uninstall(&self, home: &Path) -> anyhow::Result<()> {
        let plist = plist_path_for(home);

        if !self.unload()? {
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

    fn restart(&self) -> anyhow::Result<()> {
        let status = std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &self.service_target()])
            .status()
            .context("failed to run launchctl kickstart")?;
        if !status.success() {
            anyhow::bail!("launchctl kickstart failed with {status}");
        }
        let expected = crate::build_id::compute();
        super::wait_until_daemon_ready(&self.socket_path, expected.as_ref())
            .context("daemon did not become ready after restart")?;
        Ok(())
    }
}

/// Generate the launchd plist XML for the capsule daemon.
///
/// Uses socket activation: launchd creates the socket and launches the daemon
/// on first connection. The daemon retrieves the socket fd via
/// `launch_activate_socket`.
///
/// `forwarded_env` is embedded as `EnvironmentVariables` so that config file
/// resolution in the socket-activated daemon works identically to interactive
/// shell sessions.
pub(super) fn generate_plist(
    capsule_bin: &Path,
    socket_path: &Path,
    forwarded_env: &[(&str, String)],
) -> String {
    let env_section = format_environment_variables(forwarded_env);

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
    </array>{env_section}
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

/// Build the `EnvironmentVariables` plist fragment.
///
/// Returns an empty string when the slice is empty.
fn format_environment_variables(vars: &[(&str, String)]) -> String {
    use std::fmt::Write;

    if vars.is_empty() {
        return String::new();
    }

    let mut buf = String::from("\n    <key>EnvironmentVariables</key>\n    <dict>");
    for (key, value) in vars {
        let _ = write!(
            buf,
            "\n        <key>{}</key>\n        <string>{}</string>",
            escape_xml(key),
            escape_xml(value),
        );
    }
    buf.push_str("\n    </dict>");
    buf
}

fn escape_xml(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains(['&', '<', '>', '"', '\'']) {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Compute the plist path for a given home directory.
pub(super) fn plist_path_for(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{LAUNCHD_LABEL, LAUNCHD_SOCKET_NAME, generate_plist, plist_path_for};

    #[test]
    fn plist_contains_core_fields() {
        let plist = plist_without_env();

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
    fn plist_is_valid_xml() {
        let plist = plist_without_env();

        assert!(
            plist.starts_with("<?xml"),
            "plist should start with XML declaration"
        );
        assert!(plist.contains("</plist>"), "plist should be well-formed");
    }

    #[test]
    fn plist_omits_environment_variables_when_empty() {
        let plist = plist_without_env();

        assert!(
            !plist.contains("EnvironmentVariables"),
            "plist should not contain EnvironmentVariables when empty: {plist}"
        );
    }

    #[test]
    fn plist_embeds_environment_variables() {
        let bin = PathBuf::from("/usr/local/bin/capsule");
        let sock = PathBuf::from("/Users/test/.capsule/capsule.sock");
        let plist = generate_plist(
            &bin,
            &sock,
            &[("XDG_CONFIG_HOME", "/Users/test/.config".to_owned())],
        );

        assert!(
            plist.contains("EnvironmentVariables"),
            "plist should contain EnvironmentVariables: {plist}"
        );
        assert!(
            plist.contains("<key>XDG_CONFIG_HOME</key>"),
            "plist should contain XDG_CONFIG_HOME key: {plist}"
        );
        assert!(
            plist.contains("<string>/Users/test/.config</string>"),
            "plist should contain XDG_CONFIG_HOME value: {plist}"
        );
    }

    #[test]
    fn plist_has_no_inetd_compatibility() {
        let plist = plist_without_env();

        assert!(
            !plist.contains("inetdCompatibility"),
            "plist should not use inetdCompatibility"
        );
    }

    #[test]
    fn plist_path_uses_launch_agents_dir() {
        let home = PathBuf::from("/Users/test");
        let path = plist_path_for(&home);
        assert_eq!(
            path,
            PathBuf::from("/Users/test/Library/LaunchAgents/com.github.shuymn.capsule.plist")
        );
    }

    fn plist_without_env() -> String {
        let bin = PathBuf::from("/usr/local/bin/capsule");
        let sock = PathBuf::from("/Users/test/.capsule/capsule.sock");
        generate_plist(&bin, &sock, &[])
    }
}
