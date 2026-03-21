use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use capsule_protocol::{
    BuildId, HelloAck, Message, MessageReader, MessageWriter, PROTOCOL_VERSION, RenderResult,
    Request, Update,
};
use tokio::{net::UnixStream, sync::Mutex, task::JoinSet};

use super::{DaemonError, ReloadableConfig, SESSION_TTL, SharedState, prompt};
use crate::module::{
    CustomModuleInfo, GitModule, GitProvider, ModuleSpeed, ResolvedModule, check_when,
    custom::ResolvedSource, detect_module, required_env_var_names,
};

/// Per-connection context, cloned from the accept loop for each spawned handler.
pub(super) struct ConnectionCtx<G> {
    pub(super) state: Arc<Mutex<SharedState>>,
    pub(super) home_dir: Arc<PathBuf>,
    pub(super) git_provider: G,
    pub(super) build_id: Arc<Option<BuildId>>,
    pub(super) config: Arc<Mutex<ReloadableConfig>>,
}

struct RequestCtx<G> {
    state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    home_dir: Arc<PathBuf>,
    git_provider: G,
    config: Arc<Mutex<ReloadableConfig>>,
}

async fn write_message(
    writer: &Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    message: &Message,
) -> Result<(), DaemonError> {
    let mut locked = writer.lock().await;
    locked.write_message(message).await?;
    drop(locked);
    Ok(())
}

pub(super) async fn handle_connection<G: GitProvider + Clone + Send + 'static>(
    stream: UnixStream,
    ctx: ConnectionCtx<G>,
) -> Result<(), DaemonError> {
    let (reader, writer) = stream.into_split();
    let mut msg_reader = MessageReader::new(reader);
    let msg_writer = Arc::new(Mutex::new(MessageWriter::new(writer)));

    loop {
        match msg_reader.read_message().await {
            Ok(Some(Message::Request(req))) => {
                let req_ctx = RequestCtx {
                    state: Arc::clone(&ctx.state),
                    writer: Arc::clone(&msg_writer),
                    home_dir: Arc::clone(&ctx.home_dir),
                    git_provider: ctx.git_provider.clone(),
                    config: Arc::clone(&ctx.config),
                };
                handle_request(req, req_ctx).await?;
            }
            Ok(Some(Message::Hello(_))) => {
                let modules = {
                    let mut config = ctx.config.lock().await;
                    let (_, modules) = config.snapshot(&ctx.state).await;
                    drop(config);
                    modules
                };
                let ack = HelloAck {
                    version: PROTOCOL_VERSION,
                    build_id: (*ctx.build_id).clone(),
                    env_var_names: required_env_var_names(&modules),
                };
                write_message(&msg_writer, &Message::HelloAck(ack)).await?;
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "protocol error, closing connection");
                break;
            }
        }
    }

    Ok(())
}

/// Input for parallel custom module detection.
struct DetectInput<'a> {
    modules: &'a [ResolvedModule],
    cwd: &'a Path,
    env_vars: &'a [(String, String)],
    path_env: Option<&'a str>,
    speed: ModuleSpeed,
    timeout: Duration,
}

fn should_detect_inline(speed: ModuleSpeed, module: &ResolvedModule) -> bool {
    speed == ModuleSpeed::Fast
        && module
            .sources
            .iter()
            .all(|source| matches!(source, ResolvedSource::Env { .. }))
}

/// Detect custom modules in parallel with a timeout.
///
/// Pre-allocates slots in definition order. Each module's detection runs in a
/// separate blocking task. On timeout, remaining tasks are aborted and their
/// segments are omitted (fail-open).
async fn detect_custom_modules(input: &DetectInput<'_>) -> Vec<CustomModuleInfo> {
    // Filter matching modules (fast, no I/O)
    let matching: Vec<(usize, &ResolvedModule)> = input
        .modules
        .iter()
        .filter(|d| d.speed == input.speed)
        .filter(|d| check_when(&d.when, input.cwd, input.env_vars))
        .enumerate()
        .collect();

    if matching.is_empty() {
        return Vec::new();
    }

    let slot_count = matching.len();
    let mut slots: Vec<Option<CustomModuleInfo>> = vec![None; slot_count];
    let mut join_set = JoinSet::new();

    let mut blocking = Vec::new();
    for (slot, def) in matching {
        if should_detect_inline(input.speed, def) {
            slots[slot] = detect_module(def, input.cwd, input.env_vars, input.path_env);
        } else {
            blocking.push((slot, def.clone()));
        }
    }

    if blocking.is_empty() {
        return slots.into_iter().flatten().collect();
    }

    // Share immutable data across tasks via Arc to avoid per-task cloning.
    let shared_cwd = Arc::new(input.cwd.to_path_buf());
    let shared_env_vars = Arc::new(input.env_vars.to_vec());
    let shared_path_env = Arc::new(input.path_env.map(ToOwned::to_owned));

    for (slot, def) in blocking {
        let cwd = Arc::clone(&shared_cwd);
        let env_vars = Arc::clone(&shared_env_vars);
        let path_env = Arc::clone(&shared_path_env);
        join_set.spawn_blocking(move || {
            (
                slot,
                detect_module(&def, &cwd, &env_vars, path_env.as_deref()),
            )
        });
    }

    let deadline = tokio::time::Instant::now() + input.timeout;

    while !join_set.is_empty() {
        match tokio::time::timeout_at(deadline, join_set.join_next()).await {
            Ok(Some(Ok((slot, info)))) => {
                slots[slot] = info;
            }
            Ok(Some(Err(_))) => {} // task panicked
            Ok(None) => break,     // all done
            Err(_) => {
                // Timeout — abort remaining tasks, omit their segments
                join_set.abort_all();
                break;
            }
        }
    }

    slots.into_iter().flatten().collect()
}

async fn handle_request<G: GitProvider + Send + 'static>(
    req: Request,
    ctx: RequestCtx<G>,
) -> Result<(), DaemonError> {
    let session_id = req.session_id;
    let generation = req.generation;
    let cwd = req.cwd.clone();
    let cols = req.cols;
    let (config, modules) = {
        let mut reloadable = ctx.config.lock().await;
        reloadable.snapshot(&ctx.state).await
    };

    {
        let mut state = ctx.state.lock().await;
        state.sessions.prune_stale(SESSION_TTL);
        if !state.sessions.check_generation(session_id, generation) {
            tracing::debug!(session_id = %session_id, generation, "stale generation, discarding");
            return Ok(());
        }
    }

    let cwd_path = PathBuf::from(&cwd);
    let env_vars = req.env_vars;
    let render_ctx = crate::module::RenderContext {
        cwd: &cwd_path,
        home_dir: ctx.home_dir.as_ref().as_path(),
        last_exit_code: req.last_exit_code,
        duration_ms: req.duration_ms,
        keymap: &req.keymap,
        cols,
    };

    // Parallel fast custom module detection (runs concurrently with built-in
    // fast modules which are computed synchronously below).
    let fast_custom = detect_custom_modules(&DetectInput {
        modules: &modules,
        cwd: &cwd_path,
        env_vars: &env_vars,
        path_env: None,
        speed: ModuleSpeed::Fast,
        timeout: Duration::from_millis(config.timeout.fast_ms),
    })
    .await;

    let fast = prompt::run_fast_modules(&render_ctx, &config, fast_custom);

    let cached_slow = {
        let mut state = ctx.state.lock().await;
        state.cache.get(&cwd).cloned()
    };

    let lines = prompt::compose_prompt(&fast, cached_slow.as_ref(), usize::from(cols), &config);
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
    write_message(&ctx.writer, &Message::RenderResult(result)).await?;

    let path_env = env_vars
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.clone());

    let sent_left1 = lines.left1;
    let sent_left2 = lines.left2;
    let slow_config = Arc::clone(&config);
    let slow_modules = Arc::clone(&modules);
    let state = Arc::clone(&ctx.state);
    let writer = Arc::clone(&ctx.writer);
    tokio::spawn(async move {
        let slow_timeout = Duration::from_millis(slow_config.timeout.slow_ms);
        let deadline = tokio::time::Instant::now() + slow_timeout;

        // Single PathBuf allocation shared by git and custom module tasks.
        let cwd_path = PathBuf::from(&cwd);

        // Spawn git module task
        let git_cwd = cwd_path.clone();
        let indicator_color = slow_config.git.indicator_color;
        let git_provider = ctx.git_provider;
        let git_path_env = path_env.clone();
        let mut git_set = JoinSet::new();
        git_set.spawn_blocking(move || {
            let module = GitModule::with_indicator_color(git_provider, indicator_color);
            module
                .render_for_cwd(&git_cwd, git_path_env.as_deref())
                .map(|output| output.content)
        });
        let slow_detect_input = DetectInput {
            modules: &slow_modules,
            cwd: &cwd_path,
            env_vars: &env_vars,
            path_env: path_env.as_deref(),
            speed: ModuleSpeed::Slow,
            timeout: slow_timeout,
        };
        let custom_future = detect_custom_modules(&slow_detect_input);

        // Run git and custom detection concurrently with shared timeout
        let (git_result, custom_modules) = tokio::join!(
            async {
                match tokio::time::timeout_at(deadline, git_set.join_next()).await {
                    Ok(Some(Ok(git))) => git,
                    _ => None,
                }
            },
            custom_future,
        );

        let slow = prompt::SlowOutput {
            git: git_result,
            custom_modules,
        };

        {
            let mut state = state.lock().await;
            let session = state.sessions.get_or_create(session_id);
            if session.last_generation() != Some(generation) {
                return;
            }
            state.cache.insert(cwd, slow.clone());
        }

        let new_lines = prompt::compose_prompt(&fast, Some(&slow), usize::from(cols), &slow_config);
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
            if let Err(e) = write_message(&writer, &Message::Update(update)).await {
                tracing::debug!(session_id = %session_id, error = %e, "failed to send update");
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use capsule_protocol::{BuildId, Message, MessageReader, MessageWriter, PROTOCOL_VERSION};
    use tokio::{
        net::unix::{OwnedReadHalf, OwnedWriteHalf},
        time::sleep,
    };

    use super::super::test_support::{MockGitProvider, TestHarness, make_request, test_sid};
    use crate::{
        config::{Config, ModuleDef, ModuleWhen, SourceDef, TimeoutConfig},
        module::GitStatus,
    };

    const HOT_RELOAD_WAIT: Duration = Duration::from_millis(20);

    fn write_config(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    async fn rewrite_config(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        sleep(HOT_RELOAD_WAIT).await;
        write_config(path, content)
    }

    fn character_config(glyph: &str) -> String {
        format!("[character]\nglyph = \"{glyph}\"\n")
    }

    async fn request_left2(
        reader: &mut MessageReader<OwnedReadHalf>,
        writer: &mut MessageWriter<OwnedWriteHalf>,
        generation: u64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        writer
            .write_message(&Message::Request(make_request("/tmp", generation, 80)))
            .await?;

        match reader.read_message().await? {
            Some(Message::RenderResult(rr)) => Ok(rr.left2),
            other => Err(format!("expected RenderResult, got {other:?}").into()),
        }
    }

    async fn request_left1(
        reader: &mut MessageReader<OwnedReadHalf>,
        writer: &mut MessageWriter<OwnedWriteHalf>,
        generation: u64,
        env_vars: Vec<(String, String)>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut req = make_request("/tmp", generation, 80);
        req.env_vars = env_vars;
        writer.write_message(&Message::Request(req)).await?;

        match reader.read_message().await? {
            Some(Message::RenderResult(rr)) => Ok(rr.left1),
            other => Err(format!("expected RenderResult, got {other:?}").into()),
        }
    }

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

        let req = make_request("/tmp", 1, 80);
        writer.write_message(&Message::Request(req)).await?;

        let resp = reader.read_message().await?;
        assert!(
            matches!(&resp, Some(Message::RenderResult(_))),
            "expected RenderResult: {resp:?}"
        );

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
    async fn test_daemon_discards_stale_generation() -> Result<(), Box<dyn std::error::Error>> {
        let harness = TestHarness::start(MockGitProvider { status: None }).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 5, 80)))
            .await?;
        let r1 = reader.read_message().await?;
        match &r1 {
            Some(Message::RenderResult(rr)) => assert_eq!(rr.generation, 5),
            other => return Err(format!("expected RenderResult(gen=5), got {other:?}").into()),
        }

        writer
            .write_message(&Message::Request(make_request("/tmp", 3, 80)))
            .await?;
        writer
            .write_message(&Message::Request(make_request("/tmp", 6, 80)))
            .await?;

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

        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;

        let r1 = reader.read_message().await?;
        assert!(
            matches!(&r1, Some(Message::RenderResult(_))),
            "expected RenderResult: {r1:?}"
        );

        let u1 = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert!(
            matches!(&u1, Some(Message::Update(_))),
            "expected Update: {u1:?}"
        );

        writer
            .write_message(&Message::Request(make_request("/tmp", 2, 80)))
            .await?;

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
    async fn test_daemon_responds_to_hello_with_hello_ack() -> Result<(), Box<dyn std::error::Error>>
    {
        let build_id = BuildId::new("12345:1700000000000000000".to_owned());
        let harness = TestHarness::start_with_build_id(
            MockGitProvider { status: None },
            Some(build_id.clone()),
        )
        .await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let hello = capsule_protocol::Hello {
            version: PROTOCOL_VERSION,
            build_id: Some(BuildId::new("other-build-id".to_owned())),
        };
        writer.write_message(&Message::Hello(hello)).await?;

        let resp = reader.read_message().await?;
        match resp {
            Some(Message::HelloAck(ack)) => {
                assert_eq!(ack.version, PROTOCOL_VERSION);
                assert_eq!(ack.build_id, Some(build_id));
            }
            other => return Err(format!("expected HelloAck, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_fast_env_module_renders_without_blocking_pool()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = Config {
            module: vec![ModuleDef {
                name: "profile".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("CAPSULE_PROFILE".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                color: None,
                connector: None,
            }],
            timeout: TimeoutConfig {
                fast_ms: 0,
                ..TimeoutConfig::default()
            },
            ..Config::default()
        };
        let harness =
            TestHarness::start_with_config(MockGitProvider { status: None }, config).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let left1 = request_left1(
            &mut reader,
            &mut writer,
            1,
            vec![("CAPSULE_PROFILE".to_owned(), "dev".to_owned())],
        )
        .await?;
        assert!(
            left1.contains("dev"),
            "env-only fast module should appear in RenderResult: {left1}"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_hot_reload_uses_updated_config_on_next_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.toml");
        write_config(&config_path, &character_config("$"))?;

        let harness = TestHarness::start_with_config_path(
            MockGitProvider { status: None },
            crate::config::load_config(&config_path),
            config_path.clone(),
        )
        .await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let first = request_left2(&mut reader, &mut writer, 1).await?;
        assert!(
            first.contains('$'),
            "left2 should use initial glyph: {first}"
        );

        rewrite_config(&config_path, &character_config(">")).await?;

        let second = request_left2(&mut reader, &mut writer, 2).await?;
        assert!(
            second.contains('>'),
            "left2 should use reloaded glyph: {second}"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_hot_reload_parse_error_keeps_previous_valid_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.toml");
        write_config(&config_path, &character_config("$"))?;

        let harness = TestHarness::start_with_config_path(
            MockGitProvider { status: None },
            crate::config::load_config(&config_path),
            config_path.clone(),
        )
        .await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let first = request_left2(&mut reader, &mut writer, 1).await?;
        assert!(
            first.contains('$'),
            "left2 should use initial glyph: {first}"
        );

        rewrite_config(&config_path, "[character]\nglyph = [\n").await?;

        let second = request_left2(&mut reader, &mut writer, 2).await?;
        assert!(
            second.contains('$'),
            "parse error should keep previous glyph: {second}"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_hot_reload_loads_config_created_after_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.toml");

        let harness = TestHarness::start_with_config_path(
            MockGitProvider { status: None },
            crate::config::Config::default(),
            config_path.clone(),
        )
        .await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let first = request_left2(&mut reader, &mut writer, 1).await?;
        assert!(
            first.contains('\u{276f}'),
            "left2 should use default glyph before config exists: {first}"
        );

        rewrite_config(&config_path, &character_config(">")).await?;

        let second = request_left2(&mut reader, &mut writer, 2).await?;
        assert!(
            second.contains('>'),
            "created config should be loaded: {second}"
        );

        harness.shutdown().await
    }
}
