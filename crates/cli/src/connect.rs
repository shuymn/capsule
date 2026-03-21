//! `capsule connect` — coproc relay between stdin/stdout and the daemon socket.

use std::{
    io::{BufRead as _, Write as _},
    path::Path,
    time::Duration,
};

use anyhow::Context as _;
use capsule_protocol::{Hello, Message, PROTOCOL_VERSION};

use crate::daemon::{lock_path, socket_path};

/// Run the connect relay.
///
/// Auto-starts the daemon if it is not already running, negotiates
/// build ID to detect stale daemons, then relays bytes bidirectionally
/// between stdin/stdout and the daemon's Unix socket.
///
/// The relay automatically reconnects when the daemon connection drops,
/// retrying up to [`MAX_RETRIES`] times before exiting.
///
/// # Errors
///
/// Returns an error if the daemon cannot be started or the relay fails.
pub fn run() -> anyhow::Result<()> {
    let socket_path = socket_path()?;

    ensure_daemon(&socket_path)?;

    if !negotiate_build_id(&socket_path).unwrap_or(false) {
        restart_daemon(&socket_path, &lock_path()?)?;
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let result = rt.block_on(relay(&socket_path));

    // Forcefully shut down the runtime to avoid hanging on
    // tokio::io::stdin()'s internal blocking thread, which may be stuck
    // in a read() syscall after the parent shell closes the pipe.
    rt.shutdown_timeout(Duration::from_millis(100));

    result
}

/// Ensure the daemon is running. Auto-start if needed.
fn ensure_daemon(socket_path: &Path) -> anyhow::Result<()> {
    // Try connecting to check if daemon is alive
    if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
        return Ok(());
    }

    // Spawn daemon process
    let exe = std::env::current_exe().context("cannot find capsule binary")?;
    std::process::Command::new(&exe)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to start daemon")?;

    // Wait for socket to become available
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(10));
        if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("daemon failed to start within 1s")
}

/// Negotiate build ID with the daemon.
///
/// Returns `Ok(true)` if build IDs match (or negotiation is skipped),
/// `Ok(false)` if they differ, or an error on I/O failure.
fn negotiate_build_id(socket_path: &Path) -> anyhow::Result<bool> {
    let Some(my_build_id) = crate::build_id::compute() else {
        return Ok(true); // skip if we can't compute
    };

    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)
        .context("failed to connect for build_id negotiation")?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    // Send Hello
    let hello = Message::Hello(Hello {
        version: PROTOCOL_VERSION,
        build_id: my_build_id.clone(),
    });
    let mut wire = hello.to_wire();
    wire.push(b'\n');
    stream.write_all(&wire)?;

    // Read HelloAck
    let mut reader = std::io::BufReader::new(&stream);
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf)?;

    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.is_empty() {
        return Ok(false); // EOF — old daemon closed connection
    }

    match Message::from_wire(&buf) {
        Ok(Message::HelloAck(ack)) => {
            // Empty build_id means daemon couldn't compute — skip negotiation
            Ok(ack.build_id.is_empty() || ack.build_id == my_build_id)
        }
        _ => Ok(false),
    }
}

/// Restart the daemon by sending SIGTERM and re-launching.
fn restart_daemon(socket_path: &Path, lock_path: &Path) -> anyhow::Result<()> {
    if let Ok(pid_str) = std::fs::read_to_string(lock_path) {
        let pid_str = pid_str.trim();
        if !pid_str.is_empty() {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", pid_str])
                .status();

            // Wait for daemon to shut down (socket becomes unavailable)
            for _ in 0..100 {
                std::thread::sleep(Duration::from_millis(10));
                if std::os::unix::net::UnixStream::connect(socket_path).is_err() {
                    break;
                }
            }
        }
    }

    ensure_daemon(socket_path)
}

/// Maximum reconnection attempts before giving up.
const MAX_RETRIES: u32 = 10;

/// Delay between reconnection attempts.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Bidirectional relay: stdin ↔ socket ↔ stdout.
///
/// Automatically reconnects when the daemon connection drops (socket EOF
/// or write error), retrying up to [`MAX_RETRIES`] times with
/// [`RETRY_INTERVAL`] delay. Exits normally when stdin reaches EOF
/// (shell closed the pipe).
async fn relay(socket_path: &Path) -> anyhow::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut retries: u32 = 0;

    loop {
        // Connect to daemon, retrying on failure.
        let stream = loop {
            match tokio::net::UnixStream::connect(socket_path).await {
                Ok(s) => {
                    retries = 0;
                    break s;
                }
                Err(e) if retries >= MAX_RETRIES => return Err(e.into()),
                Err(_) => {
                    retries += 1;
                    reconnect_daemon(socket_path).await;
                }
            }
        };

        let (mut sock_read, mut sock_write) = stream.into_split();

        tokio::select! {
            result = tokio::io::copy(&mut stdin, &mut sock_write) => {
                match result {
                    Ok(_) => return Ok(()),
                    Err(e) if is_socket_error(&e) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            result = tokio::io::copy(&mut sock_read, &mut stdout) => {
                let _ = result;
            }
        }

        retries += 1;
        if retries >= MAX_RETRIES {
            return Ok(());
        }
        reconnect_daemon(socket_path).await;
    }
}

/// Wait briefly, then ensure the daemon is running for reconnection.
async fn reconnect_daemon(socket_path: &Path) {
    tokio::time::sleep(RETRY_INTERVAL).await;
    let path = socket_path.to_owned();
    let _ = tokio::task::spawn_blocking(move || ensure_daemon(&path)).await;
}

/// Returns `true` if the I/O error indicates the socket peer disconnected.
fn is_socket_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
    )
}
