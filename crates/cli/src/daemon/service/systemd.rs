//! Linux systemd service manager.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use anyhow::Context as _;

use super::ServiceManager;

/// Linux systemd user-session service manager.
pub struct Systemd {
    socket_path: PathBuf,
}

impl Systemd {
    /// Create a new [`Systemd`] service manager.
    pub fn new(socket_path: &Path) -> Self {
        Self {
            socket_path: socket_path.to_path_buf(),
        }
    }
}

impl ServiceManager for Systemd {
    fn install(&self, home: &Path, socket_path: &Path) -> anyhow::Result<()> {
        let capsule_bin = std::env::current_exe().context("cannot find capsule binary")?;
        let forwarded_env = super::collect_forwarded_env();

        let service_content = generate_service_unit(&capsule_bin, &forwarded_env);
        let socket_content = generate_socket_unit(socket_path);

        let service_file = service_file_path(home);
        let socket_file = socket_file_path(home);

        let service_unchanged = std::fs::read_to_string(&service_file)
            .is_ok_and(|existing| existing == service_content);
        let socket_unchanged =
            std::fs::read_to_string(&socket_file).is_ok_and(|existing| existing == socket_content);

        if service_unchanged && socket_unchanged {
            if super::daemon_needs_restart(&self.socket_path) {
                self.restart()?;
                println!("daemon restarted (binary updated)");
            } else {
                println!("unit files are already current, no reload needed");
            }
            return Ok(());
        }

        // Stop before rewriting unit files.
        let _ = systemctl(&["stop", "capsule.socket", "capsule.service"]);

        let unit_dir = unit_dir(home);
        std::fs::create_dir_all(&unit_dir)
            .with_context(|| format!("failed to create {}", unit_dir.display()))?;

        std::fs::write(&service_file, &service_content)
            .with_context(|| format!("failed to write {}", service_file.display()))?;
        std::fs::write(&socket_file, &socket_content)
            .with_context(|| format!("failed to write {}", socket_file.display()))?;

        systemctl(&["daemon-reload"]).context("systemctl daemon-reload failed")?;
        systemctl(&["enable", "--now", "capsule.socket"])
            .context("systemctl enable --now capsule.socket failed")?;

        super::wait_until_daemon_ready(&self.socket_path, None)
            .context("daemon did not become ready after install")?;

        println!("capsule daemon installed and started");
        Ok(())
    }

    fn uninstall(&self, home: &Path) -> anyhow::Result<()> {
        systemctl(&["stop", "capsule.socket", "capsule.service"])
            .context("systemctl stop failed")?;
        systemctl(&["disable", "capsule.socket", "capsule.service"])
            .context("systemctl disable failed")?;

        for path in [service_file_path(home), socket_file_path(home)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to remove {}", path.display()));
                }
            }
        }

        systemctl(&["daemon-reload"]).context("systemctl daemon-reload failed")?;

        println!("capsule daemon uninstalled");
        Ok(())
    }

    fn restart(&self) -> anyhow::Result<()> {
        systemctl(&["restart", "capsule.service"]).context("systemctl restart failed")?;
        let expected = crate::build_id::compute();
        super::wait_until_daemon_ready(&self.socket_path, expected.as_ref())
            .context("daemon did not become ready after restart")?;
        Ok(())
    }
}

/// Run `systemctl --user <args>`.
fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("failed to run systemctl")?;
    if !status.success() {
        anyhow::bail!("systemctl --user {} failed with {status}", args.join(" "));
    }
    Ok(())
}

/// Generate the `.service` unit file content.
fn generate_service_unit(capsule_bin: &Path, forwarded_env: &[(&str, String)]) -> String {
    let mut env_lines = String::new();
    for (key, value) in forwarded_env {
        let _ = writeln!(env_lines, "Environment={key}={value}");
    }

    format!(
        "[Unit]\n\
         Description=capsule prompt daemon\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} daemon\n\
         {env_lines}\
         [Install]\n\
         WantedBy=default.target\n",
        capsule_bin.display(),
    )
}

/// Generate the `.socket` unit file content.
fn generate_socket_unit(socket_path: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=capsule prompt daemon socket\n\
         \n\
         [Socket]\n\
         ListenStream={}\n\
         SocketMode=0700\n\
         \n\
         [Install]\n\
         WantedBy=sockets.target\n",
        socket_path.display(),
    )
}

/// `~/.config/systemd/user/` directory.
fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}

/// Path to the `.service` unit file.
fn service_file_path(home: &Path) -> PathBuf {
    unit_dir(home).join("capsule.service")
}

/// Path to the `.socket` unit file.
fn socket_file_path(home: &Path) -> PathBuf {
    unit_dir(home).join("capsule.socket")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{generate_service_unit, generate_socket_unit, service_file_path, socket_file_path};

    #[test]
    fn service_unit_contains_core_fields() {
        let unit = service_unit_without_env();

        assert!(unit.contains("[Unit]"), "should have [Unit] section");
        assert!(unit.contains("[Service]"), "should have [Service] section");
        assert!(unit.contains("[Install]"), "should have [Install] section");
        assert!(
            unit.contains("ExecStart=/usr/local/bin/capsule daemon"),
            "should have ExecStart with binary path"
        );
        assert!(unit.contains("Type=simple"), "should declare service type");
        assert!(
            unit.contains("WantedBy=default.target"),
            "should install into default.target"
        );
    }

    #[test]
    fn service_unit_embeds_environment_variables() {
        let bin = PathBuf::from("/usr/bin/capsule");
        let unit = generate_service_unit(
            &bin,
            &[("XDG_CONFIG_HOME", "/home/user/.config".to_owned())],
        );

        assert!(
            unit.contains("Environment=XDG_CONFIG_HOME=/home/user/.config"),
            "should contain forwarded env var: {unit}"
        );
    }

    #[test]
    fn service_unit_omits_environment_lines_when_empty() {
        let unit = service_unit_without_env();

        assert!(
            !unit.contains("Environment="),
            "should not contain Environment= when empty: {unit}"
        );
    }

    #[test]
    fn socket_unit_contains_core_fields() {
        let unit = socket_unit();

        assert!(unit.contains("[Unit]"), "should have [Unit] section");
        assert!(unit.contains("[Socket]"), "should have [Socket] section");
        assert!(unit.contains("[Install]"), "should have [Install] section");
        assert!(
            unit.contains("ListenStream=/home/user/.capsule/capsule.sock"),
            "should contain socket path"
        );
        assert!(
            unit.contains("SocketMode=0700"),
            "should set socket permissions"
        );
        assert!(
            unit.contains("WantedBy=sockets.target"),
            "should install into sockets.target"
        );
    }

    #[test]
    fn service_file_path_uses_systemd_user_dir() {
        let home = PathBuf::from("/home/user");
        let path = service_file_path(&home);
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/systemd/user/capsule.service")
        );
    }

    #[test]
    fn socket_file_path_uses_systemd_user_dir() {
        let home = PathBuf::from("/home/user");
        let path = socket_file_path(&home);
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/systemd/user/capsule.socket")
        );
    }

    fn service_unit_without_env() -> String {
        let bin = PathBuf::from("/usr/local/bin/capsule");
        generate_service_unit(&bin, &[])
    }

    fn socket_unit() -> String {
        let sock = PathBuf::from("/home/user/.capsule/capsule.sock");
        generate_socket_unit(&sock)
    }
}
