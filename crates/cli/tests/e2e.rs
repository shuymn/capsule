//! End-to-end tests for the capsule prompt engine.
//!
//! Tests the full flow: daemon start → connect → request → response → shutdown.

mod common;

use std::{
    io::{BufRead as _, Write as _},
    process::{Command, Stdio},
    time::Duration,
};

use capsule_protocol::{
    Hello, Message, MessageReader, MessageWriter, PROTOCOL_VERSION, PromptGeneration,
};
use common::{DaemonProcess, make_request, test_session_id, wait_for_socket_accept};

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
            assert_eq!(rr.generation, PromptGeneration::new(1));
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
            assert_eq!(rr.generation, PromptGeneration::new(2));
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
    assert!(
        wait_for_socket_accept(&daemon.socket_path, 200, Duration::from_millis(10)),
        "daemon should accept connections after stale socket recovery",
    );

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

/// `capsule connect` shall translate tab-separated text from stdin into
/// netstring wire-format for the daemon, and translate the daemon's
/// netstring responses back to tab-separated text on stdout.
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

    // Build tab-separated request and write to child's stdin
    // Format: <gen>\t<exit>\t<dur>\t<cwd>\t<cols>\t<keymap>\t<env_meta>\n
    let cwd = daemon.tmpdir_path().to_string_lossy().into_owned();
    let tab_req = format!("1\t0\t\t{cwd}\t80\tmain\t\n");
    child_stdin.write_all(tab_req.as_bytes())?;
    child_stdin.flush()?;

    // Read response from child's stdout (LF-delimited)
    let mut reader = std::io::BufReader::new(child_stdout);

    // Use a timeout thread to avoid hanging forever.
    // First line is env var metadata ("E:..."), second line is tab-separated response.
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        // Skip env var metadata line
        let mut meta_buf = Vec::new();
        let _ = reader.read_until(b'\n', &mut meta_buf);
        // Read actual tab-separated response
        let mut resp_buf = String::new();
        let n = reader.read_line(&mut resp_buf);
        let _ = tx.send((resp_buf, n));
    });

    let (resp_line, n) = rx.recv_timeout(Duration::from_secs(5))?;
    handle.join().map_err(|_panic| "reader thread panicked")?;

    let n = n?;
    assert!(n > 0, "should receive response bytes");

    // Parse tab-separated response: <type>\t<gen>\t<left1>\t<left2>\n
    let resp_line = resp_line.trim_end_matches('\n');
    let fields: Vec<&str> = resp_line.splitn(4, '\t').collect();
    assert!(
        fields.len() >= 4,
        "expected 4 tab-separated fields, got {}: {resp_line:?}",
        fields.len()
    );
    assert_eq!(fields[0], "R", "expected RenderResult type");
    assert_eq!(fields[1], "1", "expected generation 1");
    assert!(!fields[2].is_empty(), "left1 should not be empty");

    // Close stdin to let connect exit
    drop(child_stdin);
    child.wait()?;

    daemon.stop()?;
    Ok(())
}

/// When daemon is killed during an active relay, capsule connect shall
/// reconnect (via `ensure_daemon`) and resume translating messages.
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
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(child_stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // Skip env var metadata lines
                    if line.starts_with("E:") {
                        continue;
                    }
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let cwd = daemon.tmpdir_path().to_string_lossy().into_owned();

    // Phase 1: Verify relay works with tab-separated text
    let tab_req1 = format!("1\t0\t\t{cwd}\t80\tmain\t\n");
    child_stdin.write_all(tab_req1.as_bytes())?;
    child_stdin.flush()?;

    let resp_line = rx.recv_timeout(Duration::from_secs(5))?;
    let resp_line = resp_line.trim_end_matches('\n');
    let fields: Vec<&str> = resp_line.splitn(4, '\t').collect();
    assert_eq!(fields[0], "R", "expected RenderResult");
    assert_eq!(fields[1], "1", "expected generation 1");

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
    let tab_req2 = format!("2\t0\t\t{cwd}\t80\tmain\t\n");
    child_stdin.write_all(tab_req2.as_bytes())?;
    child_stdin.flush()?;

    // Read until we get RenderResult with generation 2
    let mut got_gen2 = false;
    for _ in 0..10 {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(line) => {
                let line = line.trim_end_matches('\n');
                let fields: Vec<&str> = line.splitn(4, '\t').collect();
                if fields.len() >= 2 && fields[0] == "R" && fields[1] == "2" {
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
