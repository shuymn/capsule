use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    time::Duration,
};

use capsule_protocol::{PROTOCOL_VERSION, Request, SessionId};

pub(crate) struct DaemonProcess {
    child: Option<Child>,
    pub(crate) socket_path: PathBuf,
    tmpdir: tempfile::TempDir,
}

impl DaemonProcess {
    pub(crate) fn start() -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(tempfile::tempdir()?, None)
    }

    pub(crate) fn start_with_log_level(
        log_level: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(tempfile::tempdir()?, Some(log_level))
    }

    pub(crate) fn start_in(tmpdir: tempfile::TempDir) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(tmpdir, None)
    }

    fn start_inner(
        tmpdir: tempfile::TempDir,
        log_level: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let socket_path = tmpdir.path().join("capsule.sock");
        let capsule_bin = env!("CARGO_BIN_EXE_capsule");

        let mut cmd = Command::new(capsule_bin);
        cmd.arg("daemon")
            .env("CAPSULE_SOCK_DIR", tmpdir.path())
            .env("TMPDIR", tmpdir.path())
            .env("HOME", tmpdir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Some(level) = log_level {
            cmd.env("CAPSULE_LOG", level);
        }

        let child = cmd.spawn()?;

        if wait_for_socket_accept(&socket_path, 200, Duration::from_millis(10)) {
            return Ok(Self {
                child: Some(child),
                socket_path,
                tmpdir,
            });
        }

        Err("daemon did not start within 2s".into())
    }

    pub(crate) fn tmpdir_path(&self) -> &Path {
        self.tmpdir.path()
    }

    pub(crate) fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut child) = self.child.take() {
            kill_and_wait(&mut child)?;
        }
        Ok(())
    }
}

pub(crate) fn wait_for_socket_accept(socket_path: &Path, attempts: usize, delay: Duration) -> bool {
    for _ in 0..attempts {
        if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
            return true;
        }
        std::thread::sleep(delay);
    }
    false
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = kill_and_wait(&mut child);
        }
    }
}

fn kill_and_wait(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()?;
    child.wait()?;
    Ok(())
}

pub(crate) fn test_session_id() -> SessionId {
    SessionId::from_bytes([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22])
}

pub(crate) fn make_request(cwd: &str, generation: u64) -> Request {
    Request {
        version: PROTOCOL_VERSION,
        session_id: test_session_id(),
        generation,
        cwd: cwd.to_owned(),
        cols: 80,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".to_owned(),
        env_vars: vec![],
    }
}
