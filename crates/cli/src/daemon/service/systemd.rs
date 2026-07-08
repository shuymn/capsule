//! Linux systemd service manager.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use anyhow::Context as _;

use super::ServiceManager;

const SERVICE_UNIT: &str = "capsule.service";
const SOCKET_UNIT: &str = "capsule.socket";
const SYSTEMD_USER_DIR: &str = "systemd/user";

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
    fn install(&self, home: &Path, socket_path: &Path) -> anyhow::Result<super::InstallOutcome> {
        if let Some(path) = nix_managed_service_file_path(home) {
            anyhow::bail!(
                "capsule daemon is managed by Nix at {}; change your Nix configuration instead of running `capsule daemon install`",
                path.display()
            );
        }

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
                return Ok(super::InstallOutcome::Restarted);
            }
            return Ok(super::InstallOutcome::AlreadyCurrent);
        }

        // Stop before rewriting unit files.
        let _ = systemctl(&["stop", SOCKET_UNIT, SERVICE_UNIT]);

        let unit_dir = unit_dir(home);
        std::fs::create_dir_all(&unit_dir)
            .with_context(|| format!("failed to create {}", unit_dir.display()))?;

        std::fs::write(&service_file, &service_content)
            .with_context(|| format!("failed to write {}", service_file.display()))?;
        std::fs::write(&socket_file, &socket_content)
            .with_context(|| format!("failed to write {}", socket_file.display()))?;

        systemctl(&["daemon-reload"]).context("systemctl daemon-reload failed")?;
        systemctl(&["enable", "--now", SOCKET_UNIT])
            .context("systemctl enable --now capsule.socket failed")?;

        super::wait_until_daemon_ready(&self.socket_path, None)
            .context("daemon did not become ready after install")?;

        Ok(super::InstallOutcome::Installed)
    }

    fn uninstall(&self, home: &Path) -> anyhow::Result<()> {
        if let Some(path) = nix_managed_service_file_path(home) {
            anyhow::bail!(
                "capsule daemon is managed by Nix at {}; disable the Nix module instead of running `capsule daemon uninstall`",
                path.display()
            );
        }

        systemctl(&["stop", SOCKET_UNIT, SERVICE_UNIT]).context("systemctl stop failed")?;
        systemctl(&["disable", SOCKET_UNIT, SERVICE_UNIT]).context("systemctl disable failed")?;

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
        systemctl(&["restart", SERVICE_UNIT]).context("systemctl restart failed")?;
        let expected = crate::build_id::compute();
        super::wait_until_daemon_ready(&self.socket_path, expected.as_ref())
            .context("daemon did not become ready after restart")?;
        Ok(())
    }
}

/// Run `systemctl --user <args>`.
fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let status = systemctl_command(args)
        .status()
        .context("failed to run systemctl")?;
    if !status.success() {
        anyhow::bail!("systemctl --user {} failed with {status}", args.join(" "));
    }
    Ok(())
}

fn systemctl_command(args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("systemctl");
    command.arg("--user").args(args);
    command
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

/// Return the Nix-managed service definition path, if one is present.
pub(super) fn nix_managed_service_file_path(home: &Path) -> Option<PathBuf> {
    service_definition_paths(home)
        .into_iter()
        .find(|path| super::is_nix_managed_definition(path))
}

/// Candidate service definition paths owned by capsule, Home Manager, or NixOS.
fn service_definition_paths(home: &Path) -> Vec<PathBuf> {
    service_definition_paths_with_env(
        home,
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        systemctl_fragment_path(),
    )
}

fn service_definition_paths_with_env(
    home: &Path,
    xdg_config_home: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    fragment_path: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_unique_path(&mut paths, service_file_path(home));

    if let Some(fragment_path) = fragment_path {
        push_unique_path(&mut paths, fragment_path);
    }

    if let Some(xdg_config_home) = xdg_config_home {
        push_unique_path(
            &mut paths,
            xdg_config_home.join(SYSTEMD_USER_DIR).join(SERVICE_UNIT),
        );
    }

    push_unique_path(
        &mut paths,
        data_unit_dir(home, xdg_data_home).join(SERVICE_UNIT),
    );
    push_unique_path(
        &mut paths,
        PathBuf::from("/etc")
            .join(SYSTEMD_USER_DIR)
            .join(SERVICE_UNIT),
    );
    paths
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn systemctl_fragment_path() -> Option<PathBuf> {
    let output = systemctl_command(&["show", SERVICE_UNIT, "--property=FragmentPath", "--value"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| {
            let path = PathBuf::from(line);
            path.is_absolute().then_some(path)
        })
}

/// `~/.config/systemd/user/` directory.
fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config").join(SYSTEMD_USER_DIR)
}

/// `$XDG_DATA_HOME/systemd/user/` directory (Home Manager's default unit
/// location), falling back to `~/.local/share/systemd/user/`.
fn data_unit_dir(home: &Path, xdg_data_home: Option<PathBuf>) -> PathBuf {
    xdg_data_home
        .unwrap_or_else(|| home.join(".local/share"))
        .join(SYSTEMD_USER_DIR)
}

/// Path to the `.service` unit file.
pub(super) fn service_file_path(home: &Path) -> PathBuf {
    unit_dir(home).join(SERVICE_UNIT)
}

/// Path to the `.socket` unit file.
fn socket_file_path(home: &Path) -> PathBuf {
    unit_dir(home).join(SOCKET_UNIT)
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

    #[test]
    fn service_definition_paths_defaults_to_home_config_and_etc() {
        let home = PathBuf::from("/home/user");
        let paths = super::service_definition_paths_with_env(&home, None, None, None);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/user/.config/systemd/user/capsule.service"),
                PathBuf::from("/home/user/.local/share/systemd/user/capsule.service"),
                PathBuf::from("/etc/systemd/user/capsule.service"),
            ]
        );
    }

    #[test]
    fn service_definition_paths_includes_xdg_config_home_override() {
        let home = PathBuf::from("/home/user");
        let paths = super::service_definition_paths_with_env(
            &home,
            Some(PathBuf::from("/custom/config")),
            None,
            None,
        );
        assert!(
            paths.contains(&PathBuf::from(
                "/custom/config/systemd/user/capsule.service"
            )),
            "should discover the XDG_CONFIG_HOME unit path: {paths:?}"
        );
    }

    #[test]
    fn service_definition_paths_uses_xdg_data_home_for_home_manager_units() {
        let home = PathBuf::from("/home/user");
        let paths = super::service_definition_paths_with_env(
            &home,
            None,
            Some(PathBuf::from("/custom/data")),
            None,
        );
        assert!(
            paths.contains(&PathBuf::from("/custom/data/systemd/user/capsule.service")),
            "should discover the XDG_DATA_HOME (Home Manager) unit path: {paths:?}"
        );
        assert!(
            !paths.contains(&PathBuf::from(
                "/home/user/.local/share/systemd/user/capsule.service"
            )),
            "an explicit XDG_DATA_HOME should replace the default data dir: {paths:?}"
        );
    }

    #[test]
    fn service_definition_paths_includes_systemctl_fragment_path() {
        let home = PathBuf::from("/home/user");
        let fragment = PathBuf::from("/nix/store/hash-capsule/systemd/user/capsule.service");
        let paths =
            super::service_definition_paths_with_env(&home, None, None, Some(fragment.clone()));
        assert!(
            paths.contains(&fragment),
            "should discover the systemctl FragmentPath unit path: {paths:?}"
        );
    }

    #[test]
    fn service_definition_paths_deduplicates_candidates() {
        let home = PathBuf::from("/home/user");
        let default_config = PathBuf::from("/home/user/.config");
        let fragment = PathBuf::from("/home/user/.config/systemd/user/capsule.service");
        let paths = super::service_definition_paths_with_env(
            &home,
            Some(default_config),
            None,
            Some(fragment.clone()),
        );
        let occurrences = paths.iter().filter(|path| **path == fragment).count();
        assert_eq!(
            occurrences, 1,
            "candidates equal to the default unit path should be de-duplicated: {paths:?}"
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
