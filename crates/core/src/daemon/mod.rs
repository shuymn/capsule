//! Daemon server — listens on a Unix socket and serves prompt requests.
//!
//! The daemon runs the request pipeline: fast modules → cache check → respond →
//! slow module recomputation → update. Sessions and a bounded cache track
//! per-client generation and slow module results.

mod cache;
pub mod listener;
mod session;

use std::{
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use cache::BoundedCache;
use capsule_protocol::{
    HelloAck, Message, MessageReader, MessageWriter, PROTOCOL_VERSION, RenderResult, Request,
    Update,
};
use listener::ListenerMode;
use session::SessionMap;
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Mutex,
};

use crate::{
    config::Config,
    module::{
        CmdDurationModule, DirectoryModule, GitModule, GitProvider, Module, RenderContext,
        ResolvedToolchain, TimeModule, ToolchainInfo, detect_toolchains, resolve_toolchains,
    },
    render::{
        PromptLines, compose_segments,
        segment::{Connector, Icon, Segment},
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
    cmd_duration: Option<String>,
    time: Option<String>,
    character: Option<String>,
    /// Carried forward for `compose_prompt()` to style the character module
    /// output (green on success, red on error).
    last_exit_code: i32,
    /// Whether the current working directory is read-only (no write permission bits).
    read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlowOutput {
    git: Option<String>,
    toolchains: Vec<ToolchainInfo>,
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
    home_dir: PathBuf,
    git_provider: G,
    build_id: String,
    listener_mode: ListenerMode,
    config: Arc<Config>,
    toolchains: Arc<Vec<ResolvedToolchain>>,
}

impl<G: GitProvider + Clone + Send + Sync + 'static> Server<G> {
    /// Creates a new server.
    ///
    /// # Parameters
    ///
    /// * `home_dir` — user's home directory (for `~` substitution)
    /// * `git_provider` — [`GitProvider`] implementation for slow module
    /// * `build_id` — binary fingerprint for Hello/HelloAck negotiation
    /// * `listener_mode` — how the listener was obtained (carries socket
    ///   path for [`ListenerMode::Bound`])
    /// * `config` — prompt configuration
    pub fn new(
        home_dir: PathBuf,
        git_provider: G,
        build_id: String,
        listener_mode: ListenerMode,
        config: Arc<Config>,
    ) -> Self {
        let toolchains = Arc::new(resolve_toolchains(&config.toolchain));
        Self {
            home_dir,
            git_provider,
            build_id,
            listener_mode,
            config,
            toolchains,
        }
    }

    /// Runs the daemon until `shutdown` resolves or (in bound mode) the
    /// socket file is removed/replaced.
    ///
    /// The caller provides a pre-acquired [`UnixListener`] (obtained via
    /// [`acquire_listener`](listener::acquire_listener)).
    ///
    /// In [`ListenerMode::Bound`], the accept loop monitors the socket
    /// file's inode at a fixed interval. If the file is deleted or
    /// replaced, the daemon shuts down without removing the (now-foreign)
    /// socket.
    ///
    /// In [`ListenerMode::Activated`], inode monitoring is skipped and
    /// the socket file is never removed (launchd owns the lifecycle).
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError`] if the listener cannot be converted to a
    /// tokio listener or on accept failure.
    pub async fn run(
        self,
        std_listener: std::os::unix::net::UnixListener,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), DaemonError> {
        let listener = UnixListener::from_std(std_listener)?;
        tracing::info!("daemon listening");

        let ctx = AcceptCtx {
            home_dir: Arc::new(self.home_dir),
            git_provider: self.git_provider,
            build_id: Arc::new(self.build_id),
            state: Arc::new(Mutex::new(SharedState::new())),
            config: self.config,
            toolchains: self.toolchains,
        };

        match self.listener_mode {
            ListenerMode::Bound(socket_path) => {
                Self::run_bound(socket_path, listener, shutdown, ctx).await
            }
            ListenerMode::Activated => Self::run_activated(listener, shutdown, ctx).await,
        }
    }
}

/// Shared state for the accept loops (avoids passing many args).
struct AcceptCtx<G> {
    home_dir: Arc<PathBuf>,
    git_provider: G,
    build_id: Arc<String>,
    state: Arc<Mutex<SharedState>>,
    config: Arc<Config>,
    toolchains: Arc<Vec<ResolvedToolchain>>,
}

impl<G> AcceptCtx<G> {
    fn spawn_handler(&self, stream: UnixStream)
    where
        G: GitProvider + Clone + Send + 'static,
    {
        let ctx = ConnectionCtx {
            state: Arc::clone(&self.state),
            home_dir: Arc::clone(&self.home_dir),
            git_provider: self.git_provider.clone(),
            build_id: Arc::clone(&self.build_id),
            config: Arc::clone(&self.config),
            toolchains: Arc::clone(&self.toolchains),
        };
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, ctx).await {
                tracing::warn!(error = %e, "client connection error");
            }
        });
    }
}

/// Per-connection context, cloned from [`AcceptCtx`] for each spawned handler.
struct ConnectionCtx<G> {
    state: Arc<Mutex<SharedState>>,
    home_dir: Arc<PathBuf>,
    git_provider: G,
    build_id: Arc<String>,
    config: Arc<Config>,
    toolchains: Arc<Vec<ResolvedToolchain>>,
}

impl<G: GitProvider + Clone + Send + Sync + 'static> Server<G> {
    /// Accept loop with inode monitoring (standalone/bound mode).
    async fn run_bound(
        socket_path: PathBuf,
        listener: UnixListener,
        shutdown: impl std::future::Future<Output = ()>,
        ctx: AcceptCtx<G>,
    ) -> Result<(), DaemonError> {
        // Record inode for orphan detection.
        let original_inode = std::fs::metadata(&socket_path)?.ino();
        let socket_path = Arc::new(socket_path);

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
                    ctx.spawn_handler(stream);
                }
                () = &mut shutdown => break,
                _ = inode_check.tick() => {
                    let path = Arc::clone(&socket_path);
                    let check = tokio::task::spawn_blocking(move || {
                        std::fs::metadata(&*path).map(|m| m.ino())
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
            let _ = std::fs::remove_file(&*socket_path);
        }
        Ok(())
    }

    /// Accept loop without inode monitoring (socket activation mode).
    async fn run_activated(
        listener: UnixListener,
        shutdown: impl std::future::Future<Output = ()>,
        ctx: AcceptCtx<G>,
    ) -> Result<(), DaemonError> {
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _) = result?;
                    tracing::debug!("client connected");
                    ctx.spawn_handler(stream);
                }
                () = &mut shutdown => break,
            }
        }

        tracing::info!("daemon shutting down");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

async fn handle_connection<G: GitProvider + Clone + Send + 'static>(
    stream: UnixStream,
    ctx: ConnectionCtx<G>,
) -> Result<(), DaemonError> {
    let (reader, writer) = stream.into_split();
    let mut msg_reader = MessageReader::new(reader);
    let msg_writer = Arc::new(Mutex::new(MessageWriter::new(writer)));

    loop {
        match msg_reader.read_message().await {
            Ok(Some(Message::Request(req))) => {
                handle_request(
                    req,
                    Arc::clone(&ctx.state),
                    Arc::clone(&msg_writer),
                    &ctx.home_dir,
                    ctx.git_provider.clone(),
                    Arc::clone(&ctx.config),
                    Arc::clone(&ctx.toolchains),
                )
                .await?;
            }
            Ok(Some(Message::Hello(_))) => {
                let ack = HelloAck {
                    version: PROTOCOL_VERSION,
                    build_id: (*ctx.build_id).clone(),
                };
                let mut w = msg_writer.lock().await;
                w.write_message(&Message::HelloAck(ack)).await?;
                drop(w);
            }
            Ok(Some(_)) => {}  // ignore other messages
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

#[allow(clippy::too_many_arguments)]
async fn handle_request<G: GitProvider + Send + 'static>(
    req: Request,
    state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    home_dir: &Path,
    git_provider: G,
    config: Arc<Config>,
    toolchains: Arc<Vec<ResolvedToolchain>>,
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
    let fast = run_fast_modules(&ctx, &config);

    // Cache lookup for slow results
    let cached_slow = {
        let mut s = state.lock().await;
        s.cache.get(&cwd).cloned()
    };

    // Compose and send RenderResult
    let lines = compose_prompt(&fast, cached_slow.as_ref(), usize::from(cols), &config);
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

    let path_env: Option<String> = req
        .env_vars
        .into_iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v);

    // Spawn slow module recomputation in background
    let sent_left1 = lines.left1;
    let sent_left2 = lines.left2;
    let slow_config = Arc::clone(&config);
    let slow_toolchains = Arc::clone(&toolchains);
    tokio::spawn(async move {
        let cwd_for_slow = PathBuf::from(&cwd);
        let indicator_color = slow_config.git.indicator_color;
        let Ok(slow) = tokio::task::spawn_blocking(move || {
            run_slow_modules(
                &cwd_for_slow,
                git_provider,
                indicator_color,
                path_env.as_deref(),
                &slow_toolchains,
            )
        })
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
        let new_lines = compose_prompt(&fast, Some(&slow), usize::from(cols), &slow_config);
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

fn run_fast_modules(ctx: &RenderContext<'_>, config: &Config) -> FastOutputs {
    let read_only = std::fs::metadata(ctx.cwd).is_ok_and(|m| m.permissions().readonly());
    let time = if config.time.enabled {
        TimeModule::with_show_seconds(config.time.show_seconds())
            .render(ctx)
            .map(|o| o.content)
    } else {
        None
    };
    FastOutputs {
        directory: DirectoryModule::new().render(ctx).map(|o| o.content),
        cmd_duration: CmdDurationModule::with_threshold(config.cmd_duration.threshold_ms)
            .render(ctx)
            .map(|o| o.content),
        time,
        character: Some(config.character.glyph.clone()),
        last_exit_code: ctx.last_exit_code,
        read_only,
    }
}

fn run_slow_modules<G: GitProvider>(
    cwd: &Path,
    provider: G,
    indicator_color: Color,
    path_env: Option<&str>,
    toolchain_defs: &[ResolvedToolchain],
) -> SlowOutput {
    let git_module = GitModule::with_indicator_color(provider, indicator_color);
    SlowOutput {
        git: git_module.render_for_cwd(cwd, path_env).map(|o| o.content),
        toolchains: detect_toolchains(toolchain_defs, cwd, path_env),
    }
}

// ---------------------------------------------------------------------------
// Prompt composition
// ---------------------------------------------------------------------------

const CONNECTOR_STYLE: Style = Style::new();

fn make_connector(word: &str) -> Connector {
    Connector {
        word: word.to_owned(),
        style: CONNECTOR_STYLE,
    }
}

fn make_icon(glyph: &str, style: Style) -> Icon {
    Icon {
        glyph: glyph.to_owned(),
        style,
    }
}

/// Prompt layout (Starship-compatible):
/// - Info line (left1):  `[directory] on [git] via [toolchain] [cmd_duration]`
/// - Input line (left2): `at [time] [character]`
fn compose_prompt(
    fast: &FastOutputs,
    slow: Option<&SlowOutput>,
    cols: usize,
    config: &Config,
) -> PromptLines {
    let dir_style = Style::new().fg(config.directory.color).bold();

    // -- Line 1: info line --
    let mut line1: Vec<Segment> = Vec::with_capacity(4);

    if let Some(ref dir) = fast.directory {
        if fast.read_only {
            // Pre-style: path in bold dir color + lock icon in red
            let lock_style = Style::new().fg(Color::Red);
            let content = format!("{} {}", dir_style.paint(dir), lock_style.paint("\u{f023}"));
            line1.push(Segment {
                content,
                connector: None,
                icon: None,
                content_style: None, // pre-styled
            });
        } else {
            line1.push(Segment {
                content: dir.clone(),
                connector: None,
                icon: None,
                content_style: Some(dir_style),
            });
        }
    }

    // Git output is already styled internally by the git module.
    if let Some(git) = slow.and_then(|s| s.git.as_deref()) {
        line1.push(Segment {
            content: git.to_owned(),
            connector: Some(make_connector(&config.connectors.git)),
            icon: Some(make_icon(&config.git.icon, Style::new().fg(Color::Magenta))),
            content_style: None, // pre-styled
        });
    }

    if let Some(tcs) = slow.map(|s| &s.toolchains) {
        for tc in tcs {
            let tc_icon = tc.icon.as_deref().map(|glyph| make_icon(glyph, tc.style));
            line1.push(Segment {
                content: tc.version.clone(),
                connector: Some(make_connector(&config.connectors.toolchain)),
                icon: tc_icon,
                content_style: Some(tc.style),
            });
        }
    }

    if let Some(ref dur) = fast.cmd_duration {
        line1.push(Segment {
            content: dur.clone(),
            connector: Some(make_connector(&config.connectors.cmd_duration)),
            icon: None,
            content_style: Some(Style::new().fg(config.cmd_duration.color)),
        });
    }

    // -- Line 2: input line --
    let mut line2: Vec<Segment> = Vec::with_capacity(2);

    if let Some(ref time) = fast.time {
        line2.push(Segment {
            content: time.clone(),
            connector: Some(make_connector(&config.connectors.time)),
            icon: None,
            content_style: Some(Style::new().fg(config.time.color)),
        });
    }

    if let Some(ref ch) = fast.character {
        let char_style = if fast.last_exit_code == 0 {
            Style::new().fg(config.character.success_color)
        } else {
            Style::new().fg(config.character.error_color)
        };
        line2.push(Segment {
            content: ch.clone(),
            connector: None,
            icon: None,
            content_style: Some(char_style),
        });
    }

    compose_segments(&line1, &line2, cols)
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
        fn status(
            &self,
            _cwd: &Path,
            _path_env: Option<&str>,
        ) -> Result<Option<GitStatus>, GitError> {
            Ok(self.status.clone())
        }
    }

    // -- Helpers --------------------------------------------------------------

    fn default_config() -> Config {
        Config::default()
    }

    fn test_sid() -> SessionId {
        SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
    }

    fn make_fast_outputs() -> FastOutputs {
        FastOutputs {
            directory: Some("/tmp".to_owned()),
            cmd_duration: None,
            time: None,
            character: Some("\u{276f}".to_owned()),
            last_exit_code: 0,
            read_only: false,
        }
    }

    fn make_slow_output() -> SlowOutput {
        SlowOutput {
            git: None,
            toolchains: vec![],
        }
    }

    fn make_toolchain_info(name: &str, version: &str) -> ToolchainInfo {
        // Resolve from built-in defs to get correct icon and style
        let defs = resolve_toolchains(&[]);
        let resolved = defs.iter().find(|d| d.name == name);
        ToolchainInfo {
            name: name.to_owned(),
            version: version.to_owned(),
            icon: resolved.and_then(|d| d.icon.clone()),
            style: resolved.map_or(Style::new().fg(Color::BrightBlack), |d| d.style),
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
            env_vars: vec![],
        }
    }

    fn contains_yellow_ansi(line: &str) -> bool {
        line.contains("\x1b[33m")
    }

    struct TestHarness {
        socket_path: PathBuf,
        _dir: tempfile::TempDir,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        server_handle: tokio::task::JoinHandle<Result<(), DaemonError>>,
    }

    impl TestHarness {
        async fn start(provider: MockGitProvider) -> Result<Self, Box<dyn std::error::Error>> {
            Self::start_with_build_id(provider, "test-build-id").await
        }

        async fn start_with_build_id(
            provider: MockGitProvider,
            build_id: &str,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let dir = tempfile::tempdir()?;
            let socket_path = dir.path().join("test.sock");
            let home = dir.path().join("home");
            std::fs::create_dir_all(&home)?;

            let listener =
                listener::acquire_listener(&listener::ListenerSource::Bind(socket_path.clone()))?;
            let server = Server::new(
                home,
                provider,
                build_id.to_owned(),
                ListenerMode::Bound(socket_path.clone()),
                Arc::new(Config::default()),
            );
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            let server_handle = tokio::spawn(async move {
                server
                    .run(listener, async {
                        let _ = shutdown_rx.await;
                    })
                    .await
            });

            // Wait for server to accept connections
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
            time: Some("14:30:45".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        // Line 1: directory only (no git, no toolchain)
        assert!(lines.left1.contains("/tmp"), "left1: {}", lines.left1);
        // Line 2: "at 14:30:45 ❯"
        assert!(
            lines.left2.contains("at"),
            "left2 should have 'at': {}",
            lines.left2
        );
        assert!(
            lines.left2.contains("14:30:45"),
            "left2 should have time: {}",
            lines.left2
        );
        assert!(
            lines.left2.contains('\u{276f}'),
            "left2 should have character: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_with_slow() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(lines.left1.contains("/tmp"), "left1: {}", lines.left1);
        assert!(
            lines.left1.contains("on"),
            "left1 should contain 'on' connector: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("main"),
            "left1 should contain branch: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_slow_none_git() {
        let fast = make_fast_outputs();
        let slow = make_slow_output();
        let without_slow = compose_prompt(&fast, None, 80, &default_config());
        let with_none_git = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert_eq!(without_slow, with_none_git);
    }

    #[test]
    fn test_daemon_compose_prompt_styled_directory() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left1.contains("\x1b[1;36m"),
            "directory should be bold cyan: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_styled_character_success() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
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
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left2.contains("\x1b[31m"),
            "character should be red on error: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_with_toolchain_version() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            toolchains: vec![make_toolchain_info("rust", "v1.82.0")],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains("via"),
            "left1 should contain 'via' connector: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("v1.82.0"),
            "left1 should contain version: {}",
            lines.left1
        );
        assert!(
            !lines.left1.contains("rust"),
            "left1 should not contain toolchain name: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_toolchain_uses_theme_color() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            toolchains: vec![make_toolchain_info("rust", "v1.82.0")],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains("\x1b[1;31m"),
            "rust toolchain should use bold red: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_no_toolchain_without_slow() {
        let fast = make_fast_outputs();
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.left1.contains("via"),
            "toolchain should not appear without slow output: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_multiple_toolchains() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            toolchains: vec![
                make_toolchain_info("rust", "v1.82.0"),
                make_toolchain_info("node", "v22.0.0"),
            ],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 120, &default_config());
        // Both versions should appear
        assert!(
            lines.left1.contains("v1.82.0"),
            "should contain rust version: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("v22.0.0"),
            "should contain node version: {}",
            lines.left1
        );
        // Both should have "via" connector
        let via_count = lines.left1.matches("via").count();
        assert_eq!(
            via_count, 2,
            "should have two 'via' connectors: {}",
            lines.left1
        );
        // Rust (bold red) should appear before node (bold green) — definition order
        let rust_pos = lines.left1.find("v1.82.0");
        let node_pos = lines.left1.find("v22.0.0");
        assert!(
            rust_pos < node_pos,
            "rust should come before node: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_empty_toolchains_vec() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            toolchains: vec![],
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            !lines.left1.contains("via"),
            "no 'via' connector with empty toolchains: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_time_on_line2() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        // Time should NOT be on line 1
        assert!(
            !lines.left1.contains("14:30:45"),
            "time should not be on line 1: {}",
            lines.left1
        );
        // Time should be on line 2 with "at" connector
        assert!(
            lines.left2.contains("14:30:45"),
            "time should be on line 2: {}",
            lines.left2
        );
        assert!(
            contains_yellow_ansi(&lines.left2),
            "time should use yellow styling: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_does_not_dim_connectors() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            ..make_fast_outputs()
        };
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            toolchains: vec![make_toolchain_info("rust", "v1.82.0")],
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains("on"),
            "git connector should be present: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("via"),
            "toolchain connector should be present: {}",
            lines.left1
        );
        assert!(
            lines.left2.contains("at"),
            "time connector should be present: {}",
            lines.left2
        );
        assert!(
            !lines.left1.contains("\x1b[90mon\x1b[0m")
                && !lines.left1.contains("\x1b[90mvia\x1b[0m"),
            "connectors should not use bright black: {}",
            lines.left1
        );
        assert!(
            !lines.left2.contains("\x1b[90mat\x1b[0m"),
            "time connector should not use bright black: {}",
            lines.left2
        );
        assert!(
            lines.left1.contains("\x1b[1;31m"),
            "rust toolchain should use bold red: {}",
            lines.left1
        );
        assert!(
            contains_yellow_ansi(&lines.left2),
            "time content should use yellow styling: {}",
            lines.left2
        );
    }

    // branch icon + cmd_duration connector tests

    #[test]
    fn test_daemon_compose_prompt_branch_icon_f418() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let lines = compose_prompt(&fast, Some(&slow), 80, &default_config());
        assert!(
            lines.left1.contains('\u{f418}'),
            "branch icon should be \\u{{f418}}: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_cmd_duration_took_connector() {
        let fast = FastOutputs {
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left1.contains("took"),
            "cmd_duration should have 'took' connector: {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("3s"),
            "cmd_duration should contain duration: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_readonly_shows_lock_icon() {
        let fast = FastOutputs {
            read_only: true,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left1.contains('\u{f023}'),
            "readonly dir should show lock icon: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_readonly_lock_styled_red() {
        let fast = FastOutputs {
            read_only: true,
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        // The lock icon portion should be styled red (ANSI \x1b[31m)
        let lock_pos = lines.left1.find('\u{f023}');
        assert!(lock_pos.is_some(), "lock icon should be present");
        // Check that red ANSI code appears before the lock icon
        let before_lock = &lines.left1[..lock_pos.unwrap_or(0)];
        assert!(
            before_lock.contains("\x1b[31m"),
            "lock icon should be styled red: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_writable_no_lock_icon() {
        let fast = make_fast_outputs(); // read_only: false
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.left1.contains('\u{f023}'),
            "writable dir should not show lock icon: {}",
            lines.left1
        );
    }

    // -- Config override tests ------------------------------------------------

    #[test]
    fn test_daemon_compose_prompt_custom_character_glyph() {
        let fast = FastOutputs {
            character: Some("$".to_owned()),
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            lines.left2.contains('$'),
            "left2 should contain custom glyph '$': {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_character_colors() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.character.success_color = Color::Magenta;
        let lines = compose_prompt(&fast, None, 80, &config);
        // Magenta = ANSI 35
        assert!(
            lines.left2.contains("\x1b[35m"),
            "character should use magenta on success: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_directory_color() {
        let fast = make_fast_outputs();
        let mut config = default_config();
        config.directory.color = Color::Green;
        let lines = compose_prompt(&fast, None, 80, &config);
        // Bold green = ANSI 1;32
        assert!(
            lines.left1.contains("\x1b[1;32m"),
            "directory should use bold green: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_connectors() {
        let fast = FastOutputs {
            time: Some("14:30:45".to_owned()),
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let mut config = default_config();
        config.connectors.git = "branch".to_owned();
        config.connectors.time = "time".to_owned();
        config.connectors.cmd_duration = "duration".to_owned();
        let lines = compose_prompt(&fast, Some(&slow), 80, &config);
        assert!(
            lines.left1.contains("branch"),
            "git connector should be 'branch': {}",
            lines.left1
        );
        assert!(
            lines.left1.contains("duration"),
            "cmd_duration connector should be 'duration': {}",
            lines.left1
        );
        assert!(
            lines.left2.contains("time"),
            "time connector should be 'time': {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_time_disabled() {
        let fast = FastOutputs {
            time: None, // time disabled via run_fast_modules
            ..make_fast_outputs()
        };
        let lines = compose_prompt(&fast, None, 80, &default_config());
        assert!(
            !lines.left2.contains("at"),
            "time connector should not appear when time is None: {}",
            lines.left2
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_git_icon() {
        let fast = make_fast_outputs();
        let slow = SlowOutput {
            git: Some("main".to_owned()),
            ..make_slow_output()
        };
        let mut config = default_config();
        config.git.icon = "\u{e0a0}".to_owned();
        let lines = compose_prompt(&fast, Some(&slow), 80, &config);
        assert!(
            lines.left1.contains('\u{e0a0}'),
            "git icon should be custom icon: {}",
            lines.left1
        );
        assert!(
            !lines.left1.contains('\u{f418}'),
            "default git icon should not appear: {}",
            lines.left1
        );
    }

    #[test]
    fn test_daemon_compose_prompt_custom_cmd_duration_color() {
        let fast = FastOutputs {
            cmd_duration: Some("3s".to_owned()),
            ..make_fast_outputs()
        };
        let mut config = default_config();
        config.cmd_duration.color = Color::Red;
        let lines = compose_prompt(&fast, None, 80, &config);
        // Red = ANSI 31
        assert!(
            lines.left1.contains("\x1b[31m"),
            "cmd_duration should use red: {}",
            lines.left1
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

        let listener =
            listener::acquire_listener(&listener::ListenerSource::Bind(socket_path.clone()))?;
        let server = Server::new(
            home,
            MockGitProvider { status: None },
            "test-build-id".to_owned(),
            ListenerMode::Bound(socket_path.clone()),
            Arc::new(Config::default()),
        );

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_handle = tokio::spawn(async move {
            server
                .run(listener, async {
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

        let listener =
            listener::acquire_listener(&listener::ListenerSource::Bind(socket_path.clone()))?;
        let server = Server::new(
            home,
            MockGitProvider { status: None },
            "test-build-id".to_owned(),
            ListenerMode::Bound(socket_path.clone()),
            Arc::new(Config::default()),
        );

        // No explicit shutdown — daemon should exit via inode check.
        let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let server_handle = tokio::spawn(async move {
            server
                .run(listener, async {
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

    #[tokio::test]
    async fn test_daemon_responds_to_hello_with_hello_ack() -> Result<(), Box<dyn std::error::Error>>
    {
        let build_id = "12345:1700000000000000000";
        let harness =
            TestHarness::start_with_build_id(MockGitProvider { status: None }, build_id).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let hello = capsule_protocol::Hello {
            version: PROTOCOL_VERSION,
            build_id: "other-build-id".to_owned(),
        };
        writer.write_message(&Message::Hello(hello)).await?;

        let resp = reader.read_message().await?;
        match resp {
            Some(Message::HelloAck(ack)) => {
                assert_eq!(ack.version, PROTOCOL_VERSION);
                assert_eq!(ack.build_id, build_id);
            }
            other => return Err(format!("expected HelloAck, got {other:?}").into()),
        }

        harness.shutdown().await
    }
}
