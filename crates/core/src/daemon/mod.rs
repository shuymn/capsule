//! Daemon server — listens on a Unix socket and serves prompt requests.
//!
//! The daemon runs the request pipeline: fast modules → cache check → respond →
//! slow module recomputation → update. Sessions and a bounded cache track
//! per-client generation and slow module results.

mod cache;
mod session;

use std::{
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use cache::BoundedCache;
use capsule_protocol::{
    Message, MessageReader, MessageWriter, PROTOCOL_VERSION, RenderResult, Request, Update,
};
use session::SessionMap;
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Mutex,
};

use crate::{
    module::{
        CharacterModule, CmdDurationModule, DirectoryModule, GitModule, GitProvider, Module,
        RenderContext, StatusModule, TimeModule, ToolchainModule,
    },
    render::{
        PromptLines, compose,
        style::{Color, Style},
    },
};

const CACHE_MAX_SIZE: usize = 64;
const CACHE_TTL: Duration = Duration::from_secs(30);
const SESSION_TTL: Duration = Duration::from_mins(30);

#[cfg(not(test))]
const INODE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const INODE_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Errors during daemon operation.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Socket lifecycle error (bind, accept, connect).
    #[error("socket: {0}")]
    Socket(#[from] std::io::Error),

    /// Wire protocol error (read, write, parse).
    #[error("protocol: {0}")]
    Protocol(#[from] capsule_protocol::ProtocolError),
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FastOutputs {
    directory: Option<String>,
    toolchain: Option<String>,
    cmd_duration: Option<String>,
    time: Option<String>,
    status: Option<String>,
    character: Option<String>,
    /// Carried forward for `compose_prompt()` to style the character module
    /// output (green on success, red on error).
    last_exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlowOutput {
    git: Option<String>,
}

struct SharedState {
    sessions: SessionMap,
    cache: BoundedCache<SlowOutput>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            sessions: SessionMap::new(),
            cache: BoundedCache::new(CACHE_MAX_SIZE, CACHE_TTL),
        }
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Daemon server that listens on a Unix domain socket.
///
/// Generic over `G` to allow injecting a mock [`GitProvider`] in tests.
pub struct Server<G> {
    socket_path: PathBuf,
    home_dir: PathBuf,
    git_provider: G,
}

impl<G: GitProvider + Clone + Send + Sync + 'static> Server<G> {
    /// Creates a new server.
    ///
    /// # Parameters
    ///
    /// * `socket_path` — path where the Unix socket will be created
    /// * `home_dir` — user's home directory (for `~` substitution)
    /// * `git_provider` — [`GitProvider`] implementation for slow module
    pub const fn new(socket_path: PathBuf, home_dir: PathBuf, git_provider: G) -> Self {
        Self {
            socket_path,
            home_dir,
            git_provider,
        }
    }

    /// Runs the daemon until `shutdown` resolves or the socket file is
    /// removed/replaced.
    ///
    /// Unconditionally removes any existing socket file, binds a new
    /// listener, and accepts connections. The caller is expected to
    /// guarantee exclusivity (e.g. via flock) so that unconditional
    /// removal is safe.
    ///
    /// The accept loop also monitors the socket file's inode every
    /// a fixed interval. If the file is deleted or replaced,
    /// the daemon shuts down without removing the (now-foreign) socket.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError`] on socket bind or accept failure.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), DaemonError> {
        // Unconditionally remove any existing socket file.
        // The caller guarantees exclusivity via flock, so no TOCTOU risk.
        match std::fs::remove_file(&self.socket_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(DaemonError::Socket(e)),
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(socket = %self.socket_path.display(), "daemon listening");

        // Record inode for orphan detection.
        let original_inode = std::fs::metadata(&self.socket_path)?.ino();

        let socket_path = self.socket_path;
        let home_dir = Arc::new(self.home_dir);
        let git_provider = self.git_provider;
        let state = Arc::new(Mutex::new(SharedState::new()));

        tokio::pin!(shutdown);

        let mut inode_check = tokio::time::interval(INODE_CHECK_INTERVAL);
        // The first tick fires immediately; consume it.
        inode_check.tick().await;

        let mut inode_mismatch = false;

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _) = result?;
                    tracing::debug!("client connected");
                    let conn_state = Arc::clone(&state);
                    let conn_home = Arc::clone(&home_dir);
                    let conn_git = git_provider.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, conn_state, conn_home, conn_git).await {
                            tracing::warn!(error = %e, "client connection error");
                        }
                    });
                }
                () = &mut shutdown => break,
                _ = inode_check.tick() => {
                    let path = socket_path.clone();
                    let check = tokio::task::spawn_blocking(move || {
                        std::fs::metadata(&path).map(|m| m.ino())
                    }).await;
                    match check {
                        Ok(Ok(ino)) if ino == original_inode => {}
                        _ => {
                            tracing::info!("socket file changed or removed, shutting down");
                            inode_mismatch = true;
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("daemon shutting down");
        if !inode_mismatch {
            let _ = std::fs::remove_file(&socket_path);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

async fn handle_connection<G: GitProvider + Clone + Send + 'static>(
    stream: UnixStream,
    state: Arc<Mutex<SharedState>>,
    home_dir: Arc<PathBuf>,
    git_provider: G,
) -> Result<(), DaemonError> {
    let (reader, writer) = stream.into_split();
    let mut msg_reader = MessageReader::new(reader);
    let msg_writer = Arc::new(Mutex::new(MessageWriter::new(writer)));

    loop {
        match msg_reader.read_message().await {
            Ok(Some(Message::Request(req))) => {
                handle_request(
                    req,
                    Arc::clone(&state),
                    Arc::clone(&msg_writer),
                    &home_dir,
                    git_provider.clone(),
                )
                .await?;
            }
            Ok(Some(_)) => {}  // ignore non-request messages
            Ok(None) => break, // EOF
            Err(e) => {
                tracing::debug!(error = %e, "protocol error, closing connection");
                break;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Request pipeline
// ---------------------------------------------------------------------------

async fn handle_request<G: GitProvider + Send + 'static>(
    req: Request,
    state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    home_dir: &Path,
    git_provider: G,
) -> Result<(), DaemonError> {
    let session_id = req.session_id;
    let generation = req.generation;
    let cwd = req.cwd.clone();
    let cols = req.cols;

    // Generation check + session pruning
    {
        let mut s = state.lock().await;
        s.sessions.prune_stale(SESSION_TTL);
        if !s.sessions.check_generation(session_id, generation) {
            tracing::debug!(session_id = %session_id, generation, "stale generation, discarding");
            return Ok(());
        }
    }

    // Run fast modules
    let cwd_path = PathBuf::from(&cwd);
    let ctx = RenderContext {
        cwd: &cwd_path,
        home_dir,
        last_exit_code: req.last_exit_code,
        duration_ms: req.duration_ms,
        keymap: &req.keymap,
        cols,
    };
    let fast = run_fast_modules(&ctx);

    // Cache lookup for slow results
    let cached_slow = {
        let mut s = state.lock().await;
        s.cache.get(&cwd).cloned()
    };

    // Compose and send RenderResult
    let lines = compose_prompt(&fast, cached_slow.as_ref(), usize::from(cols));
    let result = RenderResult {
        version: PROTOCOL_VERSION,
        session_id,
        generation,
        left1: lines.left1.clone(),
        left2: lines.left2.clone(),
    };
    tracing::debug!(
        session_id = %session_id,
        generation,
        cwd = %cwd,
        "sending RenderResult"
    );
    let mut w = writer.lock().await;
    w.write_message(&Message::RenderResult(result)).await?;
    drop(w);

    // Spawn slow module recomputation in background
    let sent_left1 = lines.left1;
    let sent_left2 = lines.left2;
    tokio::spawn(async move {
        let cwd_for_slow = PathBuf::from(&cwd);
        let Ok(slow) =
            tokio::task::spawn_blocking(move || run_slow_modules(&cwd_for_slow, git_provider))
                .await
        else {
            return;
        };

        // Update cache (only if generation is still current)
        {
            let mut s = state.lock().await;
            let session = s.sessions.get_or_create(session_id);
            if session.last_generation() != Some(generation) {
                return;
            }
            s.cache.insert(cwd, slow.clone());
        }

        // Send Update if prompt changed
        let new_lines = compose_prompt(&fast, Some(&slow), usize::from(cols));
        if new_lines.left1 != sent_left1 || new_lines.left2 != sent_left2 {
            tracing::debug!(
                session_id = %session_id,
                generation,
                "sending Update (slow modules changed prompt)"
            );
            let update = Update {
                version: PROTOCOL_VERSION,
                session_id,
                generation,
                left1: new_lines.left1,
                left2: new_lines.left2,
            };
            let mut w = writer.lock().await;
            let _ = w.write_message(&Message::Update(update)).await;
            drop(w);
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Module execution
// ---------------------------------------------------------------------------

fn run_fast_modules(ctx: &RenderContext<'_>) -> FastOutputs {
    FastOutputs {
        directory: DirectoryModule::new().render(ctx).map(|o| o.content),
        toolchain: ToolchainModule::new().render(ctx).map(|o| o.content),
        cmd_duration: CmdDurationModule::new().render(ctx).map(|o| o.content),
        time: TimeModule::new().render(ctx).map(|o| o.content),
        status: StatusModule::new().render(ctx).map(|o| o.content),
        character: CharacterModule::new().render(ctx).map(|o| o.content),
        last_exit_code: ctx.last_exit_code,
    }
}

fn run_slow_modules<G: GitProvider>(cwd: &Path, provider: G) -> SlowOutput {
    let module = GitModule::new(provider);
    SlowOutput {
        git: module.render_for_cwd(cwd).map(|o| o.content),
    }
}

// ---------------------------------------------------------------------------
// Prompt composition
// ---------------------------------------------------------------------------

/// Apply a style to an optional module output.
fn styled(value: Option<&str>, style: Style) -> Option<String> {
    value.map(|s| style.paint(s))
}

/// Prompt layout:
/// - Info line (left1):  `[directory] [git]  [toolchain] [cmd_duration] [time]`
/// - Input line (left2): `[status] [character]`
fn compose_prompt(fast: &FastOutputs, slow: Option<&SlowOutput>, cols: usize) -> PromptLines {
    let dir_styled = styled(
        fast.directory.as_deref(),
        Style::new().fg(Color::Cyan).bold(),
    );
    // Git output is already styled internally by the git module.
    let git_ref = slow.and_then(|s| s.git.as_deref());

    let dir_ref = dir_styled.as_deref();
    let info_left: Vec<&str> = [dir_ref, git_ref].into_iter().flatten().collect();

    let toolchain_styled = styled(fast.toolchain.as_deref(), Style::new().dimmed());
    let duration_styled = styled(fast.cmd_duration.as_deref(), Style::new().fg(Color::Yellow));
    let time_styled = styled(fast.time.as_deref(), Style::new().dimmed());

    let info_right_owned: Vec<String> = [toolchain_styled, duration_styled, time_styled]
        .into_iter()
        .flatten()
        .collect();
    let info_right: Vec<&str> = info_right_owned.iter().map(String::as_str).collect();

    let char_style = if fast.last_exit_code == 0 {
        Style::new().fg(Color::Green)
    } else {
        Style::new().fg(Color::Red)
    };
    let status_styled = styled(fast.status.as_deref(), Style::new().fg(Color::Red).bold());
    let char_styled = styled(fast.character.as_deref(), char_style);

    let input_left_owned: Vec<String> =
        [status_styled, char_styled].into_iter().flatten().collect();
    let input_left: Vec<&str> = input_left_owned.iter().map(String::as_str).collect();

    compose(&info_left, &info_right, &input_left, cols)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use capsule_protocol::SessionId;

    use super::*;
    use crate::module::{GitError, GitStatus};

    // -- Mock git provider ---------------------------------------------------

    #[derive(Debug, Clone)]
    struct MockGitProvider {
        status: Option<GitStatus>,
    }

    impl GitProvider for MockGitProvider {
        fn status(&self, _cwd: &Path) -> Result<Option<GitStatus>, GitError> {
            Ok(self.status.clone())
        }
    }

    // -- Helpers --------------------------------------------------------------

    fn test_sid() -> SessionId {
        SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
    }

    fn make_fast_outputs() -> FastOutputs {
        FastOutputs {
            directory: Some("/tmp".to_owned()),
            toolchain: None,
            cmd_duration: None,
            time: None,
            status: None,
            character: Some("\u{276f}".to_owned()),
            last_exit_code: 0,
        }
    }

    fn make_request(cwd: &str, generation: u64, cols: u16) -> Request {
        Request {
            version: PROTOCOL_VERSION,
            session_id: test_sid(),
            generation,
            cwd: cwd.to_owned(),
            cols,
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main".to_owned(),
        }
    }

    struct TestHarness {
        socket_path: PathBuf,
        _dir: tempfile::TempDir,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        server_handle: tokio::task::JoinHandle<Result<(), DaemonError>>,
    }

    impl TestHarness {
        async fn start(provider: MockGitProvider) -> Result<Self, Box<dyn std::error::Error>> {
            let dir = tempfile::tempdir()?;
            let socket_path = dir.path().join("test.sock");
            let home = dir.path().join("home");
            std::fs::create_dir_all(&home)?;

            let server = Server::new(socket_path.clone(), home, provider);
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            let server_handle = tokio::spawn(async move {
                server
                    .run(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
            });

            // Wait for server to bind
            tokio::time::sleep(Duration::from_millis(50)).await;

            Ok(Self {
                socket_path,
                _dir: dir,
                shutdown_tx: Some(shutdown_tx),
                server_handle,
            })
        }

        async fn connect(
            &self,
        ) -> Result<
            (
                MessageReader<tokio::net::unix::OwnedReadHalf>,
                MessageWriter<tokio::net::unix::OwnedWriteHalf>,
            ),
            Box<dyn std::error::Error>,
        > {
            let stream = UnixStream::connect(&self.socket_path).await?;
            let (reader, writer) = stream.into_split();
            Ok((MessageReader::new(reader), MessageWriter::new(writer)))
        }

        async fn shutdown(mut self) -> Result<(), Box<dyn std::error::Error>> {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
            self.server_handle.await??;
            Ok(())
        }
    }

    // -- compose_prompt unit tests -------------------------------------------

    #[test]
    fn test_daemon_compose_prompt_fast_only() {
        let fast = FastOutputs {
            time: Some("14:30".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 40);
        assert!(lines.left1.contains("/tmp"), "left1: {}", lines.left1);
        assert!(lines.left1.contains("14:30"), "left1: {}", lines.left1);
        assert!(lines.left2.contains('\u{276f}'), "left2: {}", lines.left2);
    }

    #[test]
    fn test_daemon_compose_prompt_with_slow() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main +2".to_owned()),
        };
        let lines = compose_prompt(&fast, Some(&slow), 40);
        assert!(lines.left1.contains("/tmp"), "left1: {}", lines.left1);
        assert!(
            lines.left1.contains("main"),
            "left1 should contain branch: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("+2"),
            "left1 should contain staged: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_slow_none_git() {
        let fast = make_fast_outputs();
        let slow = SlowOutput { git: None };
        let without_slow = compose_prompt(&fast, None, 40);
        let with_none_git = compose_prompt(&fast, Some(&slow), 40);
        assert_eq!(without_slow, with_none_git);
    }

    #[test]
    fn test_daemon_compose_prompt_styled_directory() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 40);
        assert!(
            lines.left1.contains("\x1b[1;36m"),
            "directory should be bold cyan: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_styled_character_success() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 40);
        assert!(
            lines.left2.contains("\x1b[32m"),
            "character should be green on success: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_styled_character_error() {
        let fast = FastOutputs {
            last_exit_code: 1,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 40);
        assert!(
            lines.left2.contains("\x1b[31m"),
            "character should be red on error: {}",
            lines.left2
        );
    }

    // -- Integration tests ----------------------------------------------------

    #[tokio::test]
    async fn test_daemon_responds_with_render_result() -> Result<(), Box<dyn std::error::Error>> {
        let harness = TestHarness::start(MockGitProvider { status: None }).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let req = make_request("/tmp", 1, 80);
        writer.write_message(&Message::Request(req)).await?;

        let resp = reader.read_message().await?;
        match resp {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.session_id, test_sid());
                assert_eq!(rr.generation, 1);
                assert!(
                    rr.left1.contains("/tmp"),
                    "left1 should contain directory: {}",
                    rr.left1
                );
                assert!(!rr.left2.is_empty(), "left2 should contain character");
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_sends_update_after_slow_module() -> Result<(), Box<dyn std::error::Error>>
    {
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                staged: 2,
                ..GitStatus::default()
            }),
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        // First request (no cache, so initial response has no git)
        let req = make_request("/tmp", 1, 80);
        writer.write_message(&Message::Request(req)).await?;

        // RenderResult (fast only)
        let resp = reader.read_message().await?;
        assert!(
            matches!(&resp, Some(Message::RenderResult(_))),
            "expected RenderResult: {resp:?}"
        );

        // Update (after slow module)
        let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        match update {
            Some(Message::Update(u)) => {
                assert_eq!(u.session_id, test_sid());
                assert_eq!(u.generation, 1);
                assert!(
                    u.left1.contains("main"),
                    "update left1 should contain branch: {}",
                    u.left1
                );
            }
            other => return Err(format!("expected Update, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_stale_socket_cleanup() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("test.sock");

        // Create an existing socket file (listener already dropped).
        // Server unconditionally removes it before binding.
        {
            let _listener = UnixListener::bind(&socket_path)?;
        }
        assert!(socket_path.exists(), "stale socket file should exist");

        let home = dir.path().join("home");
        std::fs::create_dir_all(&home)?;
        let server = Server::new(socket_path.clone(), home, MockGitProvider { status: None });

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_handle = tokio::spawn(async move {
            server
                .run(async {
                    let _ = shutdown_rx.await;
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Should be able to connect (stale socket was cleaned up)
        let stream = UnixStream::connect(&socket_path).await?;
        drop(stream);

        let _ = shutdown_tx.send(());
        server_handle.await??;
        Ok(())
    }

    #[tokio::test]
    async fn test_daemon_removes_socket_on_shutdown() -> Result<(), Box<dyn std::error::Error>> {
        let harness = TestHarness::start(MockGitProvider { status: None }).await?;
        let socket_path = harness.socket_path.clone();
        assert!(socket_path.exists(), "socket should exist while running");

        harness.shutdown().await?;
        assert!(
            !socket_path.exists(),
            "socket should be removed after shutdown"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_daemon_discards_stale_generation() -> Result<(), Box<dyn std::error::Error>> {
        let harness = TestHarness::start(MockGitProvider { status: None }).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        // gen=5 → should get RenderResult
        let req1 = make_request("/tmp", 5, 80);
        writer.write_message(&Message::Request(req1)).await?;
        let r1 = reader.read_message().await?;
        match &r1 {
            Some(Message::RenderResult(rr)) => assert_eq!(rr.generation, 5),
            other => return Err(format!("expected RenderResult(gen=5), got {other:?}").into()),
        }

        // gen=3 (stale) → discarded
        let req2 = make_request("/tmp", 3, 80);
        writer.write_message(&Message::Request(req2)).await?;

        // gen=6 → should get RenderResult
        let req3 = make_request("/tmp", 6, 80);
        writer.write_message(&Message::Request(req3)).await?;

        // Next message should be gen=6 (gen=3 was discarded)
        let r2 = reader.read_message().await?;
        match &r2 {
            Some(Message::RenderResult(rr)) => assert_eq!(rr.generation, 6),
            other => return Err(format!("expected RenderResult(gen=6), got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_uses_cached_slow_results() -> Result<(), Box<dyn std::error::Error>> {
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        // First request → RenderResult (no cache, no git) → Update (with git)
        let req1 = make_request("/tmp", 1, 80);
        writer.write_message(&Message::Request(req1)).await?;

        let r1 = reader.read_message().await?;
        assert!(
            matches!(&r1, Some(Message::RenderResult(_))),
            "expected RenderResult: {r1:?}"
        );

        // Wait for Update (cache is populated before Update is sent)
        let u1 = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert!(
            matches!(&u1, Some(Message::Update(_))),
            "expected Update: {u1:?}"
        );

        // Second request to same cwd → should have git from cache
        let req2 = make_request("/tmp", 2, 80);
        writer.write_message(&Message::Request(req2)).await?;

        let r2 = reader.read_message().await?;
        match &r2 {
            Some(Message::RenderResult(rr)) => {
                assert!(
                    rr.left1.contains("main"),
                    "cached response should contain git branch: {}",
                    rr.left1
                );
            }
            other => {
                return Err(format!("expected RenderResult with cache hit, got {other:?}").into());
            }
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_shuts_down_on_socket_removal() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("test.sock");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home)?;

        let server = Server::new(socket_path.clone(), home, MockGitProvider { status: None });

        // No explicit shutdown — daemon should exit via inode check.
        let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let server_handle = tokio::spawn(async move {
            server
                .run(async {
                    let _ = shutdown_rx.await;
                })
                .await
        });

        // Wait for server to bind.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Remove the socket file.
        std::fs::remove_file(&socket_path)?;

        // Server should shut down within the inode check interval + margin.
        let result = tokio::time::timeout(Duration::from_secs(2), server_handle).await??;
        assert!(
            result.is_ok(),
            "server should shut down cleanly: {result:?}"
        );
        Ok(())
    }
}
