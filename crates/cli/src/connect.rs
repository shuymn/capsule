//! `capsule connect` — coproc relay between stdin/stdout and the daemon socket.
//!
//! Translates between the simple tab-separated text protocol used by the shell
//! and the netstring wire protocol used by the daemon.
//!
//! Shell → connect: `<gen>\t<exit>\t<dur>\t<cwd>\t<cols>\t<keymap>\t<env_meta>\n`
//! Connect → shell: `<type>\t<gen>\t<left1>\t<left2>\n`

use std::{
    io::{BufRead as _, Read as _, Write as _},
    path::Path,
    time::Duration,
};

use anyhow::Context as _;
use capsule_protocol::{
    Hello, Message, MessageReader, MessageWriter, PROTOCOL_VERSION, PromptGeneration, Request,
    SessionId,
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

use crate::daemon::{lock_path, socket_path};

/// Run the connect relay.
///
/// Auto-starts the daemon if it is not already running, negotiates
/// build ID to detect stale daemons, then translates messages bidirectionally
/// between stdin/stdout (tab-separated text) and the daemon's Unix socket
/// (netstring wire protocol).
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

    let negotiation =
        negotiate_build_id(&socket_path).unwrap_or(BuildIdNegotiation::Incompatible {
            env_var_names: vec![],
        });

    if negotiation.needs_daemon_restart() {
        restart_daemon(&socket_path, &lock_path()?)?;
    }

    // Emit env var metadata to stdout so the zsh glue knows which
    // env vars to include in requests. Format: `E:<comma-separated names>\n`
    // The shell reads this one-time line before entering the relay loop.
    {
        let names = negotiation.env_var_names().join(",");
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "E:{names}");
        let _ = stdout.flush();
    }

    // Generate session ID for this shell session
    let session_id = generate_session_id()?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let result = rt.block_on(relay(&socket_path, session_id));

    // Forcefully shut down the runtime to avoid hanging on
    // tokio::io::stdin()'s internal blocking thread, which may be stuck
    // in a read() syscall after the parent shell closes the pipe.
    rt.shutdown_timeout(Duration::from_millis(100));

    result
}

/// Generate a random session ID by reading 8 bytes from `/dev/urandom`.
fn generate_session_id() -> anyhow::Result<SessionId> {
    let mut bytes = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .context("cannot open /dev/urandom")?
        .read_exact(&mut bytes)
        .context("cannot read from /dev/urandom")?;
    Ok(SessionId::from_bytes(bytes))
}

/// Ensure the daemon is running. Auto-start if needed.
fn ensure_daemon(socket_path: &Path) -> anyhow::Result<()> {
    // Try connecting to check if daemon is alive
    if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
        return Ok(());
    }

    // Spawn daemon process.
    // Intentionally detach: the daemon outlives this process and tracks
    // its own PID via the lock file.
    let exe = std::env::current_exe().context("cannot find capsule binary")?;
    let _child = std::process::Command::new(&exe)
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

/// Outcome of comparing this binary's build ID with the running daemon.
#[derive(Debug)]
pub enum BuildIdNegotiation {
    /// No local build fingerprint was computed; skip the check and use the daemon as-is.
    Unchecked {
        /// Environment variable names the daemon requested from the shell.
        env_var_names: Vec<String>,
    },
    /// Daemon reported a matching build (or no build ID on the wire).
    Compatible {
        /// Environment variable names the daemon requested from the shell.
        env_var_names: Vec<String>,
    },
    /// Empty or malformed `HelloAck`, wrong message type, or build ID mismatch.
    Incompatible {
        /// Environment variable names parsed from the ack when present (often empty).
        env_var_names: Vec<String>,
    },
}

impl BuildIdNegotiation {
    /// Environment variable names to forward from the shell on each request.
    #[must_use]
    pub const fn env_var_names(&self) -> &[String] {
        match self {
            Self::Unchecked { env_var_names }
            | Self::Compatible { env_var_names }
            | Self::Incompatible { env_var_names } => env_var_names.as_slice(),
        }
    }

    /// Whether the connect path should restart the daemon before relaying.
    #[must_use]
    pub const fn needs_daemon_restart(&self) -> bool {
        matches!(self, Self::Incompatible { .. })
    }
}

/// Perform a single synchronous request-response exchange with the daemon.
///
/// Connects to `socket_path`, sends `request` as a newline-delimited
/// netstring, reads one newline-delimited response, and returns the
/// parsed [`Message`].
///
/// # Errors
///
/// Returns an error if the connection, write, read, or deserialization
/// fails.
pub fn sync_request(
    socket_path: &Path,
    request: &Message,
    timeout: Duration,
) -> anyhow::Result<Message> {
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(socket_path).context("failed to connect to daemon")?;
    stream
        .set_read_timeout(Some(timeout))
        .context("failed to set socket timeout")?;
    stream
        .set_write_timeout(Some(timeout))
        .context("failed to set socket timeout")?;

    let mut wire = request.to_wire();
    wire.push(b'\n');
    (&stream)
        .write_all(&wire)
        .context("failed to send request to daemon")?;

    let mut reader = std::io::BufReader::new(&stream);
    let mut buf = Vec::with_capacity(128);
    reader
        .read_until(b'\n', &mut buf)
        .context("failed to read response from daemon")?;
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }

    Message::from_wire(&buf).map_err(|e| anyhow::anyhow!("failed to parse response: {e}"))
}

/// Negotiate build ID with the daemon and retrieve env var requirements.
///
/// Returns negotiation outcome and env var names, or an error on I/O failure.
pub fn negotiate_build_id(socket_path: &Path) -> anyhow::Result<BuildIdNegotiation> {
    let Some(my_build_id) = crate::build_id::compute() else {
        return Ok(BuildIdNegotiation::Unchecked {
            env_var_names: vec![],
        });
    };

    let hello = Message::Hello(Hello {
        version: PROTOCOL_VERSION,
        build_id: Some(my_build_id.clone()),
    });

    match sync_request(socket_path, &hello, Duration::from_secs(5))? {
        Message::HelloAck(ack) => {
            let compatible = ack.build_id.is_none_or(|id| id == my_build_id);
            Ok(if compatible {
                BuildIdNegotiation::Compatible {
                    env_var_names: ack.env_var_names,
                }
            } else {
                BuildIdNegotiation::Incompatible {
                    env_var_names: ack.env_var_names,
                }
            })
        }
        _ => Ok(BuildIdNegotiation::Incompatible {
            env_var_names: vec![],
        }),
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

/// Tab-prefixed line sent from connect back to the zsh glue for render/update rows.
#[derive(Clone, Copy, Debug)]
enum ShellTabLineKind {
    RenderResult,
    Update,
}

impl ShellTabLineKind {
    const fn as_prefix(self) -> &'static [u8] {
        match self {
            Self::RenderResult => b"R",
            Self::Update => b"U",
        }
    }
}

/// Maximum reconnection attempts before giving up.
const MAX_RETRIES: u32 = 10;

/// Delay between reconnection attempts.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Number of tab-separated fields in a shell request line.
const SHELL_REQUEST_FIELDS: usize = 7;

/// Message-aware relay: stdin (tab text) ↔ socket (netstring) ↔ stdout (tab text).
///
/// Translates between the shell's tab-separated text protocol and the daemon's
/// netstring wire protocol. Automatically reconnects when the daemon connection
/// drops, retrying up to [`MAX_RETRIES`] times with [`RETRY_INTERVAL`] delay.
/// Exits normally when stdin reaches EOF (shell closed the pipe).
async fn relay(socket_path: &Path, session_id: SessionId) -> anyhow::Result<()> {
    let mut stdin_reader = tokio::io::BufReader::new(tokio::io::stdin());
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

        let (sock_read, sock_write) = stream.into_split();
        let mut msg_reader = MessageReader::new(sock_read);
        let mut msg_writer = MessageWriter::new(sock_write);

        tokio::select! {
            result = translate_stdin_to_daemon(&mut stdin_reader, &mut msg_writer, session_id) => {
                match result {
                    Ok(()) => return Ok(()),
                    Err(e) if is_reconnectable(&e) => {}
                    Err(e) => return Err(e),
                }
            }
            result = translate_daemon_to_stdout(&mut msg_reader, &mut stdout) => {
                if let Err(e) = result
                    && !is_reconnectable(&e)
                {
                    return Err(e);
                }
            }
        }

        retries += 1;
        if retries >= MAX_RETRIES {
            return Ok(());
        }
        reconnect_daemon(socket_path).await;
    }
}

/// Read tab-separated lines from stdin, convert to netstring Request, send to daemon.
async fn translate_stdin_to_daemon<W: tokio::io::AsyncWrite + Unpin + Send>(
    stdin: &mut tokio::io::BufReader<tokio::io::Stdin>,
    writer: &mut MessageWriter<W>,
    session_id: SessionId,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(512);
    loop {
        buf.clear();
        let n = stdin.read_until(b'\n', &mut buf).await?;
        if n == 0 {
            return Ok(()); // EOF (shell closed pipe)
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }
        let req = parse_shell_request(&buf, session_id)?;
        writer.write_message(&Message::Request(req)).await?;
    }
}

/// Read netstring messages from daemon, convert to tab-separated text, send to stdout.
async fn translate_daemon_to_stdout<R: tokio::io::AsyncRead + Unpin + Send>(
    reader: &mut MessageReader<R>,
    stdout: &mut tokio::io::Stdout,
) -> anyhow::Result<()> {
    let mut line_buf = Vec::with_capacity(256);
    loop {
        let msg = reader.read_message().await?;
        match msg {
            Some(Message::RenderResult(rr)) => {
                format_tab_response(
                    &mut line_buf,
                    ShellTabLineKind::RenderResult,
                    rr.generation.get(),
                    &rr.left1,
                    &rr.left2,
                );
                stdout.write_all(&line_buf).await?;
                stdout.flush().await?;
            }
            Some(Message::Update(u)) => {
                format_tab_response(
                    &mut line_buf,
                    ShellTabLineKind::Update,
                    u.generation.get(),
                    &u.left1,
                    &u.left2,
                );
                stdout.write_all(&line_buf).await?;
                stdout.flush().await?;
            }
            Some(_) => {}          // ignore other message types
            None => return Ok(()), // socket EOF
        }
    }
}

/// Format a tab-separated response line: `<type>\t<gen>\t<left1>\t<left2>\n`
fn format_tab_response(
    buf: &mut Vec<u8>,
    kind: ShellTabLineKind,
    generation: u64,
    left1: &str,
    left2: &str,
) {
    use std::io::Write as _;
    buf.clear();
    buf.extend_from_slice(kind.as_prefix());
    buf.push(b'\t');
    let _ = write!(buf, "{generation}");
    buf.push(b'\t');
    buf.extend_from_slice(left1.as_bytes());
    buf.push(b'\t');
    buf.extend_from_slice(left2.as_bytes());
    buf.push(b'\n');
}

/// Parse a tab-separated shell request line into a [`Request`].
///
/// Format: `<gen>\t<exit>\t<dur>\t<cwd>\t<cols>\t<keymap>\t<env_meta>`
///
/// The `env_meta` field (last) may contain null bytes as separators between
/// `KEY=VALUE` pairs.
fn parse_shell_request(line: &[u8], session_id: SessionId) -> anyhow::Result<Request> {
    // Find the first 6 tab positions; everything after the 6th tab is env_meta.
    let mut tabs = [0usize; 6];
    let mut count = 0;
    for (i, &b) in line.iter().enumerate() {
        if b == b'\t' {
            if count < 6 {
                tabs[count] = i;
                count += 1;
            }
            if count == 6 {
                break;
            }
        }
    }

    if count != 6 {
        anyhow::bail!(
            "expected {SHELL_REQUEST_FIELDS} tab-separated fields, got {}",
            count + 1
        );
    }

    let f_gen = &line[..tabs[0]];
    let f_exit = &line[tabs[0] + 1..tabs[1]];
    let f_dur = &line[tabs[1] + 1..tabs[2]];
    let f_cwd = &line[tabs[2] + 1..tabs[3]];
    let f_cols = &line[tabs[3] + 1..tabs[4]];
    let f_keymap = &line[tabs[4] + 1..tabs[5]];
    let f_env = &line[tabs[5] + 1..];

    let generation = PromptGeneration::new(parse_utf8::<u64>(f_gen, "generation")?);
    let last_exit_code = parse_utf8::<i32>(f_exit, "last_exit_code")?;
    let duration_ms = if f_dur.is_empty() {
        None
    } else {
        Some(parse_utf8::<u64>(f_dur, "duration_ms")?)
    };
    let cwd = str_from_utf8(f_cwd, "cwd")?.to_owned();
    let cols = parse_utf8::<u16>(f_cols, "cols")?;
    let keymap = str_from_utf8(f_keymap, "keymap")?.to_owned();
    let env_vars = parse_env_meta(f_env);

    Ok(Request {
        version: PROTOCOL_VERSION,
        session_id,
        generation,
        cwd,
        cols,
        last_exit_code,
        duration_ms,
        keymap,
        env_vars,
    })
}

/// Parse a UTF-8 byte slice into a value via `FromStr`.
fn parse_utf8<T: std::str::FromStr>(bytes: &[u8], name: &str) -> anyhow::Result<T> {
    let s = str_from_utf8(bytes, name)?;
    s.parse::<T>()
        .map_err(|_parse_err| anyhow::anyhow!("invalid {name}: {s:?}"))
}

/// Convert a byte slice to a `&str`, returning a contextual error.
fn str_from_utf8<'a>(bytes: &'a [u8], name: &str) -> anyhow::Result<&'a str> {
    std::str::from_utf8(bytes).with_context(|| format!("{name} is not valid utf-8"))
}

/// Parse null-separated `KEY=VALUE` pairs from the `env_meta` field.
fn parse_env_meta(meta: &[u8]) -> Vec<(String, String)> {
    if meta.is_empty() {
        return Vec::new();
    }
    let mut vars = Vec::new();
    for part in meta.split(|&b| b == 0) {
        if let Some(eq_pos) = part.iter().position(|&b| b == b'=')
            && let (Ok(key), Ok(value)) = (
                std::str::from_utf8(&part[..eq_pos]),
                std::str::from_utf8(&part[eq_pos + 1..]),
            )
        {
            vars.push((key.to_owned(), value.to_owned()));
        }
    }
    vars
}

/// Wait briefly, then ensure the daemon is running for reconnection.
async fn reconnect_daemon(socket_path: &Path) {
    tokio::time::sleep(RETRY_INTERVAL).await;
    let path = socket_path.to_owned();
    let _ = tokio::task::spawn_blocking(move || ensure_daemon(&path)).await;
}

/// Returns `true` if the error indicates the socket peer disconnected
/// and the relay should attempt reconnection.
fn is_reconnectable(e: &anyhow::Error) -> bool {
    // Direct io::Error (from tokio stdin/stdout operations)
    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
        return is_socket_error(io_err);
    }
    // ProtocolError::Io (from MessageWriter/Reader)
    if let Some(capsule_protocol::ProtocolError::Io(io_err)) =
        e.downcast_ref::<capsule_protocol::ProtocolError>()
    {
        return is_socket_error(io_err);
    }
    false
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

#[cfg(test)]
mod tests {
    use capsule_protocol::PromptGeneration;

    use super::*;

    fn test_session_id() -> SessionId {
        SessionId::from_bytes([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22])
    }

    #[test]
    fn test_parse_shell_request_basic() -> Result<(), Box<dyn std::error::Error>> {
        let line = b"42\t0\t1500\t/home/user\t120\tmain\tPATH=/usr/bin";
        let req = parse_shell_request(line, test_session_id())?;
        assert_eq!(req.version, PROTOCOL_VERSION);
        assert_eq!(req.session_id, test_session_id());
        assert_eq!(req.generation, PromptGeneration::new(42));
        assert_eq!(req.last_exit_code, 0);
        assert_eq!(req.duration_ms, Some(1500));
        assert_eq!(req.cwd, "/home/user");
        assert_eq!(req.cols, 120);
        assert_eq!(req.keymap, "main");
        assert_eq!(
            req.env_vars,
            vec![("PATH".to_owned(), "/usr/bin".to_owned())]
        );
        Ok(())
    }

    #[test]
    fn test_parse_shell_request_empty_duration() -> Result<(), Box<dyn std::error::Error>> {
        let line = b"1\t127\t\t/tmp\t80\tmain\t";
        let req = parse_shell_request(line, test_session_id())?;
        assert_eq!(req.generation, PromptGeneration::new(1));
        assert_eq!(req.last_exit_code, 127);
        assert_eq!(req.duration_ms, None);
        assert_eq!(req.cwd, "/tmp");
        assert!(req.env_vars.is_empty());
        Ok(())
    }

    #[test]
    fn test_parse_shell_request_multiple_env_vars() -> Result<(), Box<dyn std::error::Error>> {
        let mut line = Vec::new();
        line.extend_from_slice(b"1\t0\t\t/tmp\t80\tmain\tPATH=/usr/bin");
        line.push(0); // null separator
        line.extend_from_slice(b"HOME=/home/user");
        let req = parse_shell_request(&line, test_session_id())?;
        assert_eq!(req.env_vars.len(), 2);
        assert_eq!(req.env_vars[0], ("PATH".to_owned(), "/usr/bin".to_owned()));
        assert_eq!(
            req.env_vars[1],
            ("HOME".to_owned(), "/home/user".to_owned())
        );
        Ok(())
    }

    #[test]
    fn test_parse_shell_request_negative_exit_code() -> Result<(), Box<dyn std::error::Error>> {
        let line = b"1\t-1\t\t/tmp\t80\tmain\t";
        let req = parse_shell_request(line, test_session_id())?;
        assert_eq!(req.last_exit_code, -1);
        Ok(())
    }

    #[test]
    fn test_parse_shell_request_wrong_field_count() {
        let line = b"1\t0\t\t/tmp";
        let result = parse_shell_request(line, test_session_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_format_tab_response() {
        let mut buf = Vec::new();
        format_tab_response(
            &mut buf,
            ShellTabLineKind::RenderResult,
            42,
            "~/project  main",
            "\u{276f}",
        );
        assert_eq!(buf, b"R\t42\t~/project  main\t\xe2\x9d\xaf\n");
    }

    #[test]
    fn test_format_tab_response_update() {
        let mut buf = Vec::new();
        format_tab_response(&mut buf, ShellTabLineKind::Update, 1, "info", "prompt");
        assert_eq!(buf, b"U\t1\tinfo\tprompt\n");
    }

    #[test]
    fn test_parse_env_meta_empty() {
        assert!(parse_env_meta(b"").is_empty());
    }

    #[test]
    fn test_parse_env_meta_single() {
        let vars = parse_env_meta(b"PATH=/usr/bin:/usr/local/bin");
        assert_eq!(vars.len(), 1);
        assert_eq!(
            vars[0],
            ("PATH".to_owned(), "/usr/bin:/usr/local/bin".to_owned())
        );
    }

    #[test]
    fn test_parse_env_meta_multiple() {
        let meta = b"PATH=/usr/bin\0HOME=/home/user";
        let vars = parse_env_meta(meta);
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn test_parse_env_meta_no_equals_dropped() {
        let vars = parse_env_meta(b"MALFORMED");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_generate_session_id_produces_valid_id() -> Result<(), Box<dyn std::error::Error>> {
        let id = generate_session_id()?;
        // Session ID should be 8 bytes, displayed as 16 hex chars
        assert_eq!(id.to_string().len(), 16);
        Ok(())
    }
}
