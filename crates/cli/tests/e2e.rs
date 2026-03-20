//! End-to-end tests for the capsule prompt engine.
//!
//! Tests the full flow: daemon start → connect → request → response → shutdown.

use std::{
    path::PathBuf,
    process::{Child, Command},
    time::Duration,
};

use capsule_protocol::{
    Message, MessageReader, MessageWriter, PROTOCOL_VERSION, Request, SessionId,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct DaemonProcess {
    child: Option<Child>,
    socket_path: PathBuf,
    tmpdir: tempfile::TempDir,
}

impl DaemonProcess {
    fn start() -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(tempfile::tempdir()?, None)
    }

    fn start_with_log_level(log_level: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(tempfile::tempdir()?, Some(log_level))
    }

    fn start_in(tmpdir: tempfile::TempDir) -> Result<Self, Box<dyn std::error::Error>> {
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
            .env("TMPDIR", tmpdir.path())
            .env("HOME", tmpdir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Some(level) = log_level {
            cmd.env("CAPSULE_LOG", level);
        }

        let child = cmd.spawn()?;

        // Wait for daemon to accept connections
        for _ in 0..200 {
            if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
                return Ok(Self {
                    child: Some(child),
                    socket_path,
                    tmpdir,
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        Err("daemon did not start within 2s".into())
    }

    fn tmpdir_path(&self) -> &std::path::Path {
        self.tmpdir.path()
    }

    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut child) = self.child.take() {
            kill_and_wait(&mut child)?;
        }
        Ok(())
    }
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

fn test_session_id() -> SessionId {
    SessionId::from_bytes([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22])
}

fn make_request(cwd: &str, generation: u64) -> Request {
    Request {
        version: PROTOCOL_VERSION,
        session_id: test_session_id(),
        generation,
        cwd: cwd.to_owned(),
        cols: 80,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// E2E Tests
// ---------------------------------------------------------------------------

/// Full flow: daemon start → connect → request → response → cwd change →
/// re-request → response change → shutdown → socket cleanup.
#[tokio::test]
async fn test_e2e_full_flow() -> Result<(), Box<dyn std::error::Error>> {
    let mut daemon = DaemonProcess::start()?;

    // Connect to daemon
    let stream = tokio::net::UnixStream::connect(&daemon.socket_path).await?;
    let (reader, writer) = stream.into_split();
    let mut reader = MessageReader::new(reader);
    let mut writer = MessageWriter::new(writer);

    // Create temp directories for different cwds
    let dir_a = daemon.tmpdir_path().join("dir_a");
    let dir_b = daemon.tmpdir_path().join("dir_b");
    std::fs::create_dir_all(&dir_a)?;
    std::fs::create_dir_all(&dir_b)?;

    // Request 1: cwd = dir_a
    let req1 = make_request(&dir_a.to_string_lossy(), 1);
    writer.write_message(&Message::Request(req1)).await?;

    let resp1 = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match &resp1 {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.session_id, test_session_id());
            assert_eq!(rr.generation, 1);
            assert!(
                rr.left1.contains("dir_a"),
                "left1 should contain dir_a: {}",
                rr.left1,
            );
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    // Request 2: cwd = dir_b (different directory)
    let req2 = make_request(&dir_b.to_string_lossy(), 2);
    writer.write_message(&Message::Request(req2)).await?;

    let resp2 = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match &resp2 {
        Some(Message::RenderResult(rr)) => {
            assert_eq!(rr.generation, 2);
            assert!(
                rr.left1.contains("dir_b"),
                "left1 should contain dir_b: {}",
                rr.left1,
            );
        }
        other => return Err(format!("expected RenderResult with dir_b, got {other:?}").into()),
    }

    // Disconnect client before stopping daemon
    drop(reader);
    drop(writer);

    // Shutdown
    daemon.stop()?;

    // Verify socket is cleaned up
    assert!(
        !daemon.socket_path.exists(),
        "socket should be removed after shutdown"
    );

    Ok(())
}

/// When daemon is stopped and restarted, stale socket shall be detected and
/// removed automatically.
#[tokio::test]
async fn test_e2e_stale_socket_recovery() -> Result<(), Box<dyn std::error::Error>> {
    let tmpdir = tempfile::tempdir()?;
    let socket_path = tmpdir.path().join("capsule.sock");

    // Create a stale socket file (listener dropped, file remains)
    {
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
    }
    assert!(socket_path.exists(), "stale socket should exist");

    // Start daemon — should detect and remove stale socket
    let mut daemon = DaemonProcess::start_in(tmpdir)?;

    // Verify daemon accepts connections
    let stream = tokio::net::UnixStream::connect(&daemon.socket_path).await?;
    drop(stream);

    daemon.stop()?;
    Ok(())
}

/// When `CAPSULE_LOG=debug` is set, the daemon shall output structured log
/// lines to `$TMPDIR/capsule.log`.
#[tokio::test]
async fn test_e2e_structured_logging() -> Result<(), Box<dyn std::error::Error>> {
    let mut daemon = DaemonProcess::start_with_log_level("debug")?;
    let log_path = daemon.tmpdir_path().join("capsule.log");

    // Connect and send a request to generate log output
    let stream = tokio::net::UnixStream::connect(&daemon.socket_path).await?;
    let (reader, writer) = stream.into_split();
    let mut reader = MessageReader::new(reader);
    let mut writer = MessageWriter::new(writer);

    let req = make_request("/tmp", 1);
    writer.write_message(&Message::Request(req)).await?;

    let _resp = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;

    drop(reader);
    drop(writer);
    daemon.stop()?;

    // Verify log file exists and has structured content
    assert!(
        log_path.exists(),
        "capsule.log should exist at {}",
        log_path.display()
    );
    let log_content = std::fs::read_to_string(&log_path)?;
    assert!(!log_content.is_empty(), "log file should not be empty");

    Ok(())
}
