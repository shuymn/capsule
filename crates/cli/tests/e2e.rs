//! End-to-end tests for the capsule prompt engine.
//!
//! Tests the full flow: daemon start → connect → request → response → shutdown.

use std::{
    io::{BufRead as _, Write as _},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

use capsule_protocol::{
    Hello, Message, MessageReader, MessageWriter, PROTOCOL_VERSION, Request, SessionId,
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
        env_vars: vec![],
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

/// `capsule connect` shall relay wire-format messages between stdin/stdout
/// and the daemon socket.
#[tokio::test]
async fn test_e2e_connect_relay() -> Result<(), Box<dyn std::error::Error>> {
    let mut daemon = DaemonProcess::start()?;

    let capsule_bin = env!("CARGO_BIN_EXE_capsule");
    let mut child = Command::new(capsule_bin)
        .arg("connect")
        .env("CAPSULE_SOCK_DIR", daemon.tmpdir_path())
        .env("TMPDIR", daemon.tmpdir_path())
        .env("HOME", daemon.tmpdir_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut child_stdin = child.stdin.take().ok_or("no stdin")?;
    let child_stdout = child.stdout.take().ok_or("no stdout")?;

    // Build wire-format Request and write to child's stdin
    let req = Message::Request(Request {
        version: PROTOCOL_VERSION,
        session_id: test_session_id(),
        generation: 1,
        cwd: daemon.tmpdir_path().to_string_lossy().into_owned(),
        cols: 80,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".to_owned(),
        env_vars: vec![],
    });
    let mut wire = req.to_wire();
    wire.push(b'\n');
    child_stdin.write_all(&wire)?;
    child_stdin.flush()?;

    // Read response from child's stdout (LF-delimited)
    let mut reader = std::io::BufReader::new(child_stdout);

    // Use a timeout thread to avoid hanging forever.
    // First line is env var metadata ("E:..."), second line is wire response.
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        // Skip env var metadata line
        let mut meta_buf = Vec::new();
        let _ = reader.read_until(b'\n', &mut meta_buf);
        // Read actual wire response
        let mut resp_buf = Vec::new();
        let n = reader.read_until(b'\n', &mut resp_buf);
        let _ = tx.send((resp_buf, n));
    });

    let (resp_buf, n) = rx.recv_timeout(Duration::from_secs(5))?;
    handle.join().map_err(|_panic| "reader thread panicked")?;

    let n = n?;
    assert!(n > 0, "should receive response bytes");

    // Strip trailing LF and parse
    let wire_data = if resp_buf.last() == Some(&b'\n') {
        &resp_buf[..resp_buf.len() - 1]
    } else {
        &resp_buf
    };
    let response = Message::from_wire(wire_data)?;

    match response {
        Message::RenderResult(rr) => {
            assert_eq!(rr.session_id, test_session_id());
            assert_eq!(rr.generation, 1);
            assert!(!rr.left1.is_empty(), "left1 should not be empty");
        }
        other => return Err(format!("expected RenderResult, got {other:?}").into()),
    }

    // Close stdin to let connect exit
    drop(child_stdin);
    child.wait()?;

    daemon.stop()?;
    Ok(())
}

/// When daemon is killed during an active relay, capsule connect shall
/// reconnect (via `ensure_daemon`) and resume relaying.
#[tokio::test]
async fn test_e2e_connect_reconnects_after_daemon_restart() -> Result<(), Box<dyn std::error::Error>>
{
    let mut daemon = DaemonProcess::start()?;
    let capsule_bin = env!("CARGO_BIN_EXE_capsule");

    // Start capsule connect
    let mut connect = Command::new(capsule_bin)
        .arg("connect")
        .env("CAPSULE_SOCK_DIR", daemon.tmpdir_path())
        .env("TMPDIR", daemon.tmpdir_path())
        .env("HOME", daemon.tmpdir_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut child_stdin = connect.stdin.take().ok_or("no stdin")?;
    let child_stdout = connect.stdout.take().ok_or("no stdout")?;

    // Background reader: collects line-delimited messages from connect stdout.
    // Skips the initial env var metadata line ("E:...").
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(child_stdout);
        loop {
            let mut buf = Vec::new();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // Skip env var metadata lines
                    if buf.starts_with(b"E:") {
                        continue;
                    }
                    if tx.send(buf).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Phase 1: Verify relay works
    let req1 = Message::Request(make_request(&daemon.tmpdir_path().to_string_lossy(), 1));
    let mut wire1 = req1.to_wire();
    wire1.push(b'\n');
    child_stdin.write_all(&wire1)?;
    child_stdin.flush()?;

    let resp_buf = rx.recv_timeout(Duration::from_secs(5))?;
    let resp_wire = if resp_buf.last() == Some(&b'\n') {
        &resp_buf[..resp_buf.len() - 1]
    } else {
        &resp_buf
    };
    match Message::from_wire(resp_wire)? {
        Message::RenderResult(rr) => assert_eq!(rr.generation, 1),
        other => return Err(format!("expected RenderResult gen 1, got {other:?}").into()),
    }

    // Phase 2: Kill daemon, let connect's relay reconnect via ensure_daemon
    daemon.stop()?;

    // Wait for relay to detect disconnect and auto-start a new daemon
    std::thread::sleep(Duration::from_secs(2));

    // Connect should still be running (reconnected)
    assert!(
        connect.try_wait()?.is_none(),
        "connect should still be running after daemon restart"
    );

    // Phase 3: Send request through reconnected relay
    let req2 = Message::Request(make_request(&daemon.tmpdir_path().to_string_lossy(), 2));
    let mut wire2 = req2.to_wire();
    wire2.push(b'\n');
    child_stdin.write_all(&wire2)?;
    child_stdin.flush()?;

    // Read until we get RenderResult with generation 2
    let mut got_gen2 = false;
    for _ in 0..10 {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(buf) => {
                let w = if buf.last() == Some(&b'\n') {
                    &buf[..buf.len() - 1]
                } else {
                    &buf
                };
                if let Ok(Message::RenderResult(rr)) = Message::from_wire(w)
                    && rr.generation == 2
                {
                    got_gen2 = true;
                    break;
                }
            }
            Err(_) => break,
        }
    }
    assert!(
        got_gen2,
        "should receive RenderResult with generation 2 after reconnection"
    );

    // Cleanup: close stdin to let connect exit
    drop(child_stdin);
    let status = connect.wait()?;
    assert!(status.success(), "connect should exit cleanly");

    // Kill daemon started by ensure_daemon (not tracked by DaemonProcess)
    let lock_path = daemon.tmpdir_path().join("capsule.lock");
    if let Ok(pid_str) = std::fs::read_to_string(&lock_path) {
        let pid_str = pid_str.trim();
        if !pid_str.is_empty() {
            let _ = Command::new("kill").args(["-TERM", pid_str]).status();
        }
    }

    Ok(())
}

/// Daemon shall respond to a Hello message with a HelloAck containing
/// its build ID.
#[tokio::test]
async fn test_e2e_hello_handshake() -> Result<(), Box<dyn std::error::Error>> {
    let mut daemon = DaemonProcess::start()?;

    let stream = tokio::net::UnixStream::connect(&daemon.socket_path).await?;
    let (reader, writer) = stream.into_split();
    let mut reader = MessageReader::new(reader);
    let mut writer = MessageWriter::new(writer);

    let hello = Hello {
        version: PROTOCOL_VERSION,
        build_id: Some(capsule_protocol::BuildId::new("test:123".to_owned())),
    };
    writer.write_message(&Message::Hello(hello)).await?;

    let resp = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
    match resp {
        Some(Message::HelloAck(ack)) => {
            assert_eq!(ack.version, PROTOCOL_VERSION);
            assert!(ack.build_id.is_some(), "daemon should return its build_id");
        }
        other => return Err(format!("expected HelloAck, got {other:?}").into()),
    }

    drop(reader);
    drop(writer);
    daemon.stop()?;
    Ok(())
}
