use std::{path::PathBuf, sync::Arc, time::Duration};

use capsule_protocol::{
    BuildId, ConfigGeneration, DepHash, HelloAck, Message, MessageReader, MessageWriter,
    PROTOCOL_VERSION, PromptGeneration, RenderResult, Request, Update,
};
use tokio::{
    net::UnixStream,
    sync::{Mutex, watch},
    task::JoinSet,
};

use super::{
    CacheKey, DaemonError, ReloadableConfig, SESSION_TTL, SharedState, prompt, session::Session,
    stats::DaemonStats,
};
use crate::module::{
    CustomModuleInfo, DetectedModuleCandidate, GitModule, GitProvider, ModuleSpeed, RequestFacts,
    ResolvedModule, ResolvedSource, arbitrate_detected_modules, required_env_var_names,
};

mod pipeline;

use pipeline::{CollectedFacts, ConfigSnapshot, GatedPromptRequest};

/// Per-connection context, cloned from the accept loop for each spawned handler.
pub(super) struct ConnectionCtx<G> {
    pub(super) state: Arc<Mutex<SharedState>>,
    pub(super) home_dir: Arc<PathBuf>,
    pub(super) git_provider: G,
    pub(super) build_id: Arc<Option<BuildId>>,
    pub(super) config: Arc<Mutex<ReloadableConfig>>,
    pub(super) stats: Arc<DaemonStats>,
}

struct RequestCtx<G> {
    state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    home_dir: Arc<PathBuf>,
    git_provider: G,
    config: Arc<Mutex<ReloadableConfig>>,
    stats: Arc<DaemonStats>,
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
                    stats: Arc::clone(&ctx.stats),
                };
                handle_request(req, req_ctx).await?;
            }
            Ok(Some(Message::StatusRequest(_))) => {
                let response = {
                    let state = ctx.state.lock().await;
                    let config = ctx.config.lock().await;
                    ctx.stats.snapshot(&state, &config)
                };
                write_message(&msg_writer, &Message::StatusResponse(response)).await?;
            }
            Ok(Some(Message::Hello(_))) => {
                let modules = {
                    let mut config = ctx.config.lock().await;
                    let (_, modules, _) = config.snapshot(&ctx.state, &ctx.stats).await;
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
    facts: Arc<RequestFacts>,
    speed: ModuleSpeed,
    timeout: Duration,
    stats: Option<Arc<DaemonStats>>,
}

fn cache_key_for_request(
    cwd: &str,
    config_generation: ConfigGeneration,
    modules: &[ResolvedModule],
    facts: &RequestFacts,
) -> CacheKey {
    let deps = facts.matching_dependency_inputs(modules, ModuleSpeed::Slow);
    let dep_hash = DepHash::new(deps.compute_dep_hash(facts));
    CacheKey::new(cwd.to_owned(), config_generation, dep_hash)
}

/// Run git status and slow custom modules concurrently, returning the combined
/// result.  Both the cache-hit revalidation and cache-miss paths call this.
async fn compute_slow_modules<G: GitProvider + Send + 'static>(
    git_provider: G,
    facts: &Arc<RequestFacts>,
    modules: &[ResolvedModule],
    config: &crate::config::Config,
    daemon_stats: &Arc<DaemonStats>,
) -> prompt::SlowOutput {
    use std::sync::atomic::Ordering;

    let slow_start = std::time::Instant::now();
    let slow_timeout = Duration::from_millis(config.timeout.slow_ms);
    let deadline = tokio::time::Instant::now() + slow_timeout;

    let mut git_set = JoinSet::new();
    if !config.git.disabled {
        let git_cwd = facts.cwd().to_path_buf();
        let git_styles = crate::module::git::GitStyles {
            branch: config.git.prompt_style(),
            detached_hash: config.git.detached_hash_prompt_style(),
            indicator: config.git.indicator_prompt_style(),
            state: config.git.state_prompt_style(),
            color_map: config.color_map,
        };
        let git_path_env = facts.command_path_env().map(ToOwned::to_owned);
        git_set.spawn_blocking(move || {
            let module = GitModule::with_styles(git_provider, git_styles);
            module
                .render_for_cwd(&git_cwd, git_path_env.as_deref())
                .map(|output| output.content)
        });
    }

    let slow_detect_input = DetectInput {
        modules,
        facts: Arc::clone(facts),
        speed: ModuleSpeed::Slow,
        timeout: slow_timeout,
        stats: Some(Arc::clone(daemon_stats)),
    };
    let custom_future = detect_custom_modules(&slow_detect_input);

    let (git_result, custom_modules) = tokio::join!(
        async {
            match tokio::time::timeout_at(deadline, git_set.join_next()).await {
                Ok(Some(Ok(git))) => git,
                Err(_) => {
                    daemon_stats.git_timeouts.fetch_add(1, Ordering::Relaxed);
                    None
                }
                _ => None,
            }
        },
        custom_future,
    );

    let elapsed_us = u64::try_from(slow_start.elapsed().as_micros()).unwrap_or(u64::MAX);
    daemon_stats
        .slow_compute_duration_us
        .fetch_add(elapsed_us, Ordering::Relaxed);

    prompt::SlowOutput {
        git: git_result,
        custom_modules,
    }
}

/// Spawn a background task that re-runs all slow modules and, if the result
/// differs from the cached version, sends an `Update` and refreshes the cache.
#[expect(
    clippy::too_many_arguments,
    reason = "all parameters are owned types forwarded into a spawned task"
)]
fn spawn_slow_revalidation<G: GitProvider + Send + 'static>(
    cached: Arc<prompt::SlowOutput>,
    git_provider: G,
    facts: Arc<RequestFacts>,
    modules: Arc<Vec<ResolvedModule>>,
    config: Arc<crate::config::Config>,
    shared_state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    daemon_stats: Arc<DaemonStats>,
    cache_key: CacheKey,
    session_id: capsule_protocol::SessionId,
    generation: PromptGeneration,
    fast: prompt::FastOutputs,
    sent_left1: String,
    sent_left2: String,
    cols: u16,
) {
    use std::sync::atomic::Ordering;

    daemon_stats
        .slow_computes_started
        .fetch_add(1, Ordering::Relaxed);

    tokio::spawn(async move {
        let updated_slow =
            compute_slow_modules(git_provider, &facts, &modules, &config, &daemon_stats).await;

        if updated_slow == *cached {
            return;
        }

        let updated_slow = Arc::new(updated_slow);
        {
            let mut state_locked = shared_state.lock().await;
            if state_locked
                .cache
                .insert(cache_key, Arc::clone(&updated_slow))
            {
                daemon_stats.cache_evictions.fetch_add(1, Ordering::Relaxed);
            }
        }

        try_send_slow_update(
            &shared_state,
            &writer,
            session_id,
            generation,
            &fast,
            &updated_slow,
            &sent_left1,
            &sent_left2,
            cols,
            &config,
        )
        .await;
    });
}

struct SlowUpdateTarget {
    state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    receiver: watch::Receiver<Option<Arc<prompt::SlowOutput>>>,
    session_id: capsule_protocol::SessionId,
    generation: PromptGeneration,
    sent_left1: String,
    sent_left2: String,
    fast: prompt::FastOutputs,
    cols: u16,
    config: Arc<crate::config::Config>,
}

fn should_detect_inline(speed: ModuleSpeed, module: &ResolvedModule) -> bool {
    speed == ModuleSpeed::Fast
        && module
            .all_sources()
            .all(|source| matches!(source, ResolvedSource::Env { .. }))
}

/// Detect custom modules in parallel with a timeout.
///
/// Pre-allocates slots in definition order. Each module's detection runs in a
/// separate blocking task. On timeout, remaining tasks are aborted and their
/// segments are omitted (fail-open).
async fn detect_custom_modules(input: &DetectInput<'_>) -> Vec<CustomModuleInfo> {
    // Filter matching modules (fast, no I/O)
    let matching = input.facts.matching_modules(input.modules, input.speed);

    if matching.is_empty() {
        return Vec::new();
    }

    let slot_count = matching.len();
    let mut slots: Vec<Option<CustomModuleInfo>> = vec![None; slot_count];
    let mut join_set = JoinSet::new();

    let mut deferred = Vec::new();
    for (slot, def) in matching.iter().copied() {
        if should_detect_inline(input.speed, def) {
            slots[slot] = input.facts.detect_module(def).await;
        } else {
            deferred.push((slot, def.clone()));
        }
    }

    if !deferred.is_empty() {
        for (slot, def) in deferred {
            let facts = Arc::clone(&input.facts);
            join_set.spawn(async move { (slot, facts.detect_module(&def).await) });
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
                    if let Some(ref stats) = input.stats {
                        stats
                            .custom_module_timeouts
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    join_set.abort_all();
                    break;
                }
            }
        }
    }

    let detected = matching_modules_to_candidates(&matching, slots);
    arbitrate_detected_modules(detected)
}

fn matching_modules_to_candidates(
    matching: &[(usize, &ResolvedModule)],
    slots: Vec<Option<CustomModuleInfo>>,
) -> Vec<DetectedModuleCandidate> {
    matching
        .iter()
        .zip(slots)
        .filter_map(|((_, def), info)| info.map(|info| DetectedModuleCandidate::new(def, info)))
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "request pipeline is staged with pipeline::* types; further extraction is optional"
)]
async fn handle_request<G: GitProvider + Send + 'static>(
    req: Request,
    ctx: RequestCtx<G>,
) -> Result<(), DaemonError> {
    use std::sync::atomic::Ordering;

    ctx.stats.requests_total.fetch_add(1, Ordering::Relaxed);

    let Request {
        session_id,
        generation,
        cwd,
        cols,
        last_exit_code,
        duration_ms,
        keymap,
        env_vars,
        version: _,
    } = req;

    let config_snap = {
        let mut reloadable = ctx.config.lock().await;
        let snapshot = reloadable.snapshot(&ctx.state, &ctx.stats).await;
        drop(reloadable);
        let (config, modules, config_generation) = snapshot;
        ConfigSnapshot {
            config,
            modules,
            config_generation,
        }
    };

    {
        let mut state = ctx.state.lock().await;
        let pruned = state.sessions.prune_stale(SESSION_TTL);
        if pruned > 0 {
            ctx.stats
                .sessions_pruned
                .fetch_add(pruned.try_into().unwrap_or(u64::MAX), Ordering::Relaxed);
        }
        if !state.sessions.check_generation(session_id, generation) {
            ctx.stats.stale_discards.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                session_id = %session_id,
                generation = generation.get(),
                "stale generation, discarding"
            );
            return Ok(());
        }
    }

    let gated = GatedPromptRequest {
        session_id,
        generation,
        cwd,
        cols,
        last_exit_code,
        duration_ms,
        keymap,
    };

    let facts = Arc::new(
        RequestFacts::collect(PathBuf::from(&gated.cwd), env_vars).with_forwarded_path_env(),
    );
    let cache_key = cache_key_for_request(
        &gated.cwd,
        config_snap.config_generation,
        &config_snap.modules,
        facts.as_ref(),
    );
    let collected = CollectedFacts { facts, cache_key };

    let render_ctx = crate::module::RenderContext {
        cwd: collected.facts.cwd(),
        home_dir: ctx.home_dir.as_ref().as_path(),
        last_exit_code: gated.last_exit_code,
        duration_ms: gated.duration_ms,
        keymap: &gated.keymap,
        cols: gated.cols,
    };

    // Parallel fast custom module detection (runs concurrently with built-in
    // fast modules which are computed synchronously below).
    let fast_custom = detect_custom_modules(&DetectInput {
        modules: &config_snap.modules,
        facts: Arc::clone(&collected.facts),
        speed: ModuleSpeed::Fast,
        timeout: Duration::from_millis(config_snap.config.timeout.fast_ms),
        stats: None,
    })
    .await;

    let fast = prompt::run_fast_modules(
        &render_ctx,
        &config_snap.config,
        collected.facts.read_only(),
        fast_custom,
    );

    let cache_enabled = config_snap.config.cache.slow != crate::config::SlowCacheMode::Off;
    let cached_slow = if cache_enabled {
        let mut state = ctx.state.lock().await;
        state.cache.get(&collected.cache_key).map(|v| {
            ctx.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            v.clone()
        })
    } else {
        None
    };
    if cached_slow.is_none() {
        ctx.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    let lines = prompt::compose_prompt(
        &fast,
        cached_slow.as_deref(),
        usize::from(gated.cols),
        &config_snap.config,
    );

    let result = RenderResult {
        version: PROTOCOL_VERSION,
        session_id: gated.session_id,
        generation: gated.generation,
        left1: lines.left1.clone(),
        left2: lines.left2.clone(),
        meta: lines.char_meta.clone(),
    };
    tracing::debug!(
        session_id = %gated.session_id,
        generation = gated.generation.get(),
        cwd = %gated.cwd,
        "sending RenderResult"
    );
    write_message(&ctx.writer, &Message::RenderResult(result)).await?;

    if let Some(cached) = cached_slow {
        // Cache hit: revalidate all slow modules in background.  If the fresh
        // result differs from the cached version, send an Update and refresh
        // the cache entry.
        spawn_slow_revalidation(
            cached,
            ctx.git_provider,
            Arc::clone(&collected.facts),
            Arc::clone(&config_snap.modules),
            Arc::clone(&config_snap.config),
            Arc::clone(&ctx.state),
            Arc::clone(&ctx.writer),
            Arc::clone(&ctx.stats),
            collected.cache_key,
            gated.session_id,
            gated.generation,
            fast.clone(),
            lines.left1,
            lines.left2,
            gated.cols,
        );

        return Ok(());
    }

    let sent_left1 = lines.left1;
    let sent_left2 = lines.left2;
    let slow_config = Arc::clone(&config_snap.config);
    let slow_modules = Arc::clone(&config_snap.modules);
    let CollectedFacts { facts, cache_key } = collected;
    let state = Arc::clone(&ctx.state);
    let writer = Arc::clone(&ctx.writer);
    let should_start_compute = {
        let mut shared_state = state.lock().await;
        let (receiver, should_start) = shared_state.inflight.get(&cache_key).cloned().map_or_else(
            || {
                let (sender, receiver) = watch::channel(None);
                shared_state.inflight.insert(cache_key.clone(), sender);
                (receiver, true)
            },
            |sender| {
                ctx.stats.inflight_coalesces.fetch_add(1, Ordering::Relaxed);
                (sender.subscribe(), false)
            },
        );
        drop(shared_state);
        tokio::spawn(wait_for_slow_update(SlowUpdateTarget {
            state: Arc::clone(&state),
            writer: Arc::clone(&writer),
            receiver,
            session_id: gated.session_id,
            generation: gated.generation,
            sent_left1: sent_left1.clone(),
            sent_left2: sent_left2.clone(),
            fast: fast.clone(),
            cols: gated.cols,
            config: Arc::clone(&slow_config),
        }));
        should_start
    };

    if !should_start_compute {
        return Ok(());
    }

    let slow_stats = Arc::clone(&ctx.stats);
    let state = Arc::clone(&ctx.state);
    slow_stats
        .slow_computes_started
        .fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        let slow = compute_slow_modules(
            ctx.git_provider,
            &facts,
            &slow_modules,
            &slow_config,
            &slow_stats,
        )
        .await;

        let slow = Arc::new(slow);
        let sender = {
            let mut state = state.lock().await;
            let sender = state.inflight.remove(&cache_key);
            if cache_enabled && state.cache.insert(cache_key, Arc::clone(&slow)) {
                slow_stats.cache_evictions.fetch_add(1, Ordering::Relaxed);
            }
            drop(state);
            sender
        };
        if let Some(sender) = sender {
            let _ = sender.send(Some(slow));
        }
    });

    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "grouping into a struct would add lifetime noise for a private helper"
)]
async fn try_send_slow_update(
    state: &Arc<Mutex<SharedState>>,
    writer: &Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    session_id: capsule_protocol::SessionId,
    generation: PromptGeneration,
    fast: &prompt::FastOutputs,
    slow: &prompt::SlowOutput,
    sent_left1: &str,
    sent_left2: &str,
    cols: u16,
    config: &crate::config::Config,
) {
    let is_current = {
        let shared = state.lock().await;
        shared
            .sessions
            .get(session_id)
            .and_then(Session::last_generation)
            == Some(generation)
    };
    if !is_current {
        return;
    }

    let new_lines = prompt::compose_prompt(fast, Some(slow), usize::from(cols), config);
    if new_lines.left1 == sent_left1 && new_lines.left2 == sent_left2 {
        return;
    }

    tracing::debug!(
        session_id = %session_id,
        generation = generation.get(),
        "sending Update (slow modules changed prompt)"
    );
    let update = Update {
        version: PROTOCOL_VERSION,
        session_id,
        generation,
        left1: new_lines.left1,
        left2: new_lines.left2,
        // char_meta depends only on config + exit_code (not slow modules),
        // so new_lines.char_meta is always correct here.
        meta: new_lines.char_meta,
    };
    if let Err(error) = write_message(writer, &Message::Update(update)).await {
        tracing::debug!(session_id = %session_id, error = %error, "failed to send update");
    }
}

async fn wait_for_slow_update(target: SlowUpdateTarget) {
    let SlowUpdateTarget {
        state,
        writer,
        mut receiver,
        session_id,
        generation,
        sent_left1,
        sent_left2,
        fast,
        cols,
        config,
    } = target;

    if receiver.changed().await.is_err() {
        return;
    }

    let slow = receiver.borrow().clone();
    let Some(slow) = slow else {
        return;
    };

    try_send_slow_update(
        &state,
        &writer,
        session_id,
        generation,
        &fast,
        &slow,
        &sent_left1,
        &sent_left2,
        cols,
        &config,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use capsule_protocol::{
        BuildId, Message, MessageReader, MessageWriter, PROTOCOL_VERSION, PromptGeneration,
        Request, SessionId,
    };
    use tokio::{
        net::unix::{OwnedReadHalf, OwnedWriteHalf},
        time::sleep,
    };

    use super::super::test_support::{
        MockGitProvider, TestHarness, make_request, make_sleep_module, test_sid,
    };
    use crate::{
        config::{
            CacheConfig, Config, ModuleDef, ModuleWhen, SlowCacheMode, SourceDef, StyleConfig,
            TimeoutConfig,
        },
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

    fn count_git_calls() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    fn other_sid() -> SessionId {
        SessionId::from_bytes([0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88])
    }

    fn make_request_with_sid(
        cwd: &str,
        session_id: SessionId,
        generation: u64,
        cols: u16,
    ) -> Request {
        Request {
            version: PROTOCOL_VERSION,
            session_id,
            generation: PromptGeneration::new(generation),
            cwd: cwd.to_owned(),
            cols,
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main".to_owned(),
            env_vars: vec![],
        }
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
        let harness = TestHarness::start(MockGitProvider::default()).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let req = make_request("/tmp", 1, 80);
        writer.write_message(&Message::Request(req)).await?;

        let resp = reader.read_message().await?;
        match resp {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.session_id, test_sid());
                assert_eq!(rr.generation, PromptGeneration::new(1));
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
            ..MockGitProvider::default()
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
                assert_eq!(u.generation, PromptGeneration::new(1));
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
        let harness = TestHarness::start(MockGitProvider::default()).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 5, 80)))
            .await?;
        let r1 = reader.read_message().await?;
        match &r1 {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.generation, PromptGeneration::new(5));
            }
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
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.generation, PromptGeneration::new(6));
            }
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
            ..MockGitProvider::default()
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
    async fn test_daemon_revalidates_git_on_cache_hit_without_update_when_unchanged()
    -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            call_count: Some(Arc::clone(&call_count)),
            ..MockGitProvider::default()
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        writer
            .write_message(&Message::Request(make_request("/tmp", 2, 80)))
            .await?;
        let second = reader.read_message().await?;
        match &second {
            Some(Message::RenderResult(rr)) => {
                assert!(
                    rr.left1.contains("main"),
                    "cache hit should render slow output"
                );
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }

        let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
        assert!(
            update.is_err(),
            "unchanged git result should not trigger Update"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "cache hit should still revalidate git in background"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_skips_git_when_disabled() -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            call_count: Some(Arc::clone(&call_count)),
            ..MockGitProvider::default()
        };
        let config = Config {
            git: crate::config::GitConfig {
                disabled: true,
                ..crate::config::GitConfig::default()
            },
            cache: CacheConfig {
                slow: SlowCacheMode::Off,
            },
            ..Config::default()
        };
        let harness = TestHarness::start_with_config(provider, config).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;

        let resp = reader.read_message().await?;
        assert!(
            matches!(&resp, Some(Message::RenderResult(_))),
            "expected RenderResult: {resp:?}"
        );

        // Caching is off so slow compute runs inline; no Update should arrive.
        let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
        assert!(update.is_err(), "no Update expected when git is disabled");

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "git provider should not be called when disabled"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_coalesces_inflight_slow_recompute_for_same_cache_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            delay: Duration::from_millis(150),
            call_count: Some(Arc::clone(&call_count)),
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;
        writer
            .write_message(&Message::Request(make_request("/tmp", 2, 80)))
            .await?;

        let mut saw_gen1 = false;
        let mut saw_gen2 = false;
        while !saw_gen1 || !saw_gen2 {
            match reader.read_message().await? {
                Some(Message::RenderResult(rr)) => {
                    saw_gen1 |= rr.generation == PromptGeneration::new(1);
                    saw_gen2 |= rr.generation == PromptGeneration::new(2);
                }
                other => {
                    return Err(
                        format!("expected RenderResult while draining, got {other:?}").into(),
                    );
                }
            }
        }

        match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
            Some(Message::Update(update)) => {
                assert_eq!(update.generation, PromptGeneration::new(2));
                assert!(update.left1.contains("main"));
            }
            other => return Err(format!("expected Update for generation 2, got {other:?}").into()),
        }

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "same cache key should share one slow recompute"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_hot_reload_does_not_reuse_stale_slow_cache_from_previous_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.toml");
        write_config(&config_path, "[git.indicator_style]\nfg = \"red\"\n")?;

        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                staged: 1,
                ..GitStatus::default()
            }),
            delay: Duration::from_millis(120),
            call_count: Some(Arc::clone(&call_count)),
        };
        let harness = TestHarness::start_with_config_path(
            provider,
            crate::config::load_config(&config_path),
            config_path.clone(),
        )
        .await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;
        let _ = reader.read_message().await?;

        rewrite_config(&config_path, "[git.indicator_style]\nfg = \"green\"\n").await?;
        sleep(Duration::from_millis(160)).await;
        let generation1 = PromptGeneration::new(1);
        let generation2 = PromptGeneration::new(2);
        if let Ok(Ok(Some(Message::Update(update)))) =
            tokio::time::timeout(Duration::from_millis(50), reader.read_message()).await
        {
            assert_eq!(update.generation, generation1);
        }

        writer
            .write_message(&Message::Request(make_request("/tmp", 2, 80)))
            .await?;
        loop {
            match reader.read_message().await? {
                Some(Message::RenderResult(rr)) => {
                    assert_eq!(rr.generation, generation2);
                    assert!(
                        !rr.left1.contains("main"),
                        "new config generation should not reuse stale slow cache: {}",
                        rr.left1
                    );
                    break;
                }
                Some(Message::Update(update)) => {
                    assert_eq!(
                        update.generation, generation1,
                        "only the prior generation may arrive before RenderResult(gen=2)"
                    );
                }
                other => return Err(format!("expected RenderResult, got {other:?}").into()),
            }
        }

        loop {
            match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
                Some(Message::Update(update)) => {
                    if update.generation == generation1 {
                        continue;
                    }
                    assert_eq!(update.generation, generation2);
                    assert!(update.left1.contains("main"));
                    assert!(
                        update.left1.contains("\x1b[32m"),
                        "updated prompt should use reloaded git style: {}",
                        update.left1
                    );
                    break;
                }
                other => return Err(format!("expected Update, got {other:?}").into()),
            }
        }

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "reloaded config should force a fresh slow recompute"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_caches_slow_results_for_same_env_dependency_value()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = Config {
            module: vec![ModuleDef {
                name: "env-sensitive".to_owned(),
                when: ModuleWhen {
                    files: vec![],
                    env: vec!["CAPSULE_PROFILE".to_owned()],
                },
                source: vec![SourceDef {
                    name: "value".to_owned(),
                    env: None,
                    file: None,
                    command: Some(vec![
                        "sh".to_owned(),
                        "-c".to_owned(),
                        "echo dynamic".to_owned(),
                    ]),
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: Some("via".to_owned()),
                arbitration: None,
            }],
            ..Config::default()
        };
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            call_count: Some(Arc::clone(&call_count)),
            ..MockGitProvider::default()
        };
        let harness = TestHarness::start_with_config(provider, config).await?;
        let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
        let (mut reader, mut writer) = harness.connect().await?;

        let mut first = make_request(&cwd, 1, 80);
        first.env_vars = vec![("CAPSULE_PROFILE".to_owned(), "dev".to_owned())];
        writer.write_message(&Message::Request(first)).await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let mut second = make_request(&cwd, 2, 80);
        second.env_vars = vec![("CAPSULE_PROFILE".to_owned(), "dev".to_owned())];
        writer.write_message(&Message::Request(second)).await?;
        let rr = reader.read_message().await?;
        match &rr {
            Some(Message::RenderResult(rr)) => {
                assert!(
                    rr.left1.contains("main"),
                    "cache hit should include slow output: {}",
                    rr.left1
                );
            }
            other => {
                return Err(format!("expected RenderResult with cache hit, got {other:?}").into());
            }
        }
        let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
        assert!(
            update.is_err(),
            "same env value should produce cache hit with no Update"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "cache hit should still revalidate git in background"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_responds_to_hello_with_hello_ack() -> Result<(), Box<dyn std::error::Error>>
    {
        let build_id = BuildId::new("12345:1700000000000000000".to_owned());
        let harness =
            TestHarness::start_with_build_id(MockGitProvider::default(), Some(build_id.clone()))
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
                    name: "value".to_owned(),
                    env: Some("CAPSULE_PROFILE".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: None,
            }],
            timeout: TimeoutConfig {
                fast_ms: 0,
                ..TimeoutConfig::default()
            },
            ..Config::default()
        };
        let harness = TestHarness::start_with_config(MockGitProvider::default(), config).await?;
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
            MockGitProvider::default(),
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
            MockGitProvider::default(),
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
            MockGitProvider::default(),
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

    #[tokio::test]
    async fn test_daemon_empty_cwd_does_not_panic() -> Result<(), Box<dyn std::error::Error>> {
        let harness = TestHarness::start(MockGitProvider::default()).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let req = make_request("", 1, 80);
        writer.write_message(&Message::Request(req)).await?;

        let resp = reader.read_message().await?;
        match resp {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.session_id, test_sid());
                assert_eq!(rr.generation, PromptGeneration::new(1));
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_path_traversal_cwd_handled_safely()
    -> Result<(), Box<dyn std::error::Error>> {
        let harness = TestHarness::start(MockGitProvider::default()).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        let req = make_request("/../../../etc/passwd", 1, 80);
        writer.write_message(&Message::Request(req)).await?;

        let resp = reader.read_message().await?;
        match resp {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.generation, PromptGeneration::new(1));
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_different_cwds_get_independent_slow_computes()
    -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            delay: Duration::from_millis(100),
            call_count: Some(Arc::clone(&call_count)),
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp/a", 1, 80)))
            .await?;
        writer
            .write_message(&Message::Request(make_request("/tmp/b", 2, 80)))
            .await?;

        let mut render_count = 0;
        while render_count < 2 {
            match reader.read_message().await? {
                Some(Message::RenderResult(_)) => render_count += 1,
                other => {
                    return Err(
                        format!("expected RenderResult while draining, got {other:?}").into(),
                    );
                }
            }
        }

        match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
            Some(Message::Update(u)) => {
                assert_eq!(
                    u.generation,
                    PromptGeneration::new(2),
                    "only latest generation should get Update"
                );
            }
            other => return Err(format!("expected Update for gen=2, got {other:?}").into()),
        }

        let extra = tokio::time::timeout(Duration::from_millis(300), reader.read_message()).await;
        assert!(extra.is_err(), "stale gen=1 Update should be suppressed");

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "different cwds must trigger independent slow computes"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_config_reload_during_inflight_does_not_poison_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_path = dir.path().join("config.toml");
        write_config(&config_path, "[git.indicator_style]\nfg = \"red\"\n")?;

        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("feature".to_owned()),
                staged: 1,
                ..GitStatus::default()
            }),
            delay: Duration::from_millis(200),
            call_count: Some(Arc::clone(&call_count)),
        };
        let harness = TestHarness::start_with_config_path(
            provider,
            crate::config::load_config(&config_path),
            config_path.clone(),
        )
        .await?;
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;
        let _ = reader.read_message().await?;

        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(&config_path, "[git.indicator_style]\nfg = \"green\"\n")?;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = tokio::time::timeout(Duration::from_millis(100), reader.read_message()).await;

        writer
            .write_message(&Message::Request(make_request("/tmp", 2, 80)))
            .await?;
        match reader.read_message().await? {
            Some(Message::RenderResult(rr)) => {
                assert_eq!(rr.generation, PromptGeneration::new(2));
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }

        match tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await?? {
            Some(Message::Update(update)) => {
                assert_eq!(update.generation, PromptGeneration::new(2));
                assert!(
                    update.left1.contains("feature"),
                    "Update should contain branch: {}",
                    update.left1
                );
                assert!(
                    update.left1.contains("\x1b[32m"),
                    "Update should use green style from reloaded config: {}",
                    update.left1
                );
            }
            other => return Err(format!("expected Update, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_cross_session_cache_sharing() -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("shared".to_owned()),
                ..GitStatus::default()
            }),
            delay: Duration::from_millis(50),
            call_count: Some(Arc::clone(&call_count)),
        };
        let harness = TestHarness::start(provider).await?;

        let (mut reader1, mut writer1) = harness.connect().await?;
        writer1
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;
        let _ = reader1.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader1.read_message()).await??;

        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let (mut reader2, mut writer2) = harness.connect().await?;
        writer2
            .write_message(&Message::Request(make_request_with_sid(
                "/tmp",
                other_sid(),
                1,
                80,
            )))
            .await?;
        let resp2 = reader2.read_message().await?;
        match resp2 {
            Some(Message::RenderResult(rr)) => {
                assert!(
                    rr.left1.contains("shared"),
                    "second session should get cached slow output: {}",
                    rr.left1
                );
            }
            other => return Err(format!("expected RenderResult, got {other:?}").into()),
        }

        let no_update =
            tokio::time::timeout(Duration::from_millis(200), reader2.read_message()).await;
        assert!(
            no_update.is_err(),
            "cache hit should not produce an Update for second session"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "cache hit should still revalidate git in background"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_rapid_generation_suppresses_stale_updates()
    -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            delay: Duration::from_millis(100),
            call_count: Some(Arc::clone(&call_count)),
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        for generation in 1..=4u64 {
            writer
                .write_message(&Message::Request(make_request("/tmp", generation, 80)))
                .await?;
        }

        for _ in 0..4 {
            match reader.read_message().await? {
                Some(Message::RenderResult(_)) => {}
                other => {
                    return Err(
                        format!("expected RenderResult while draining, got {other:?}").into(),
                    );
                }
            }
        }

        let update = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        match update {
            Some(Message::Update(u)) => {
                assert_eq!(
                    u.generation,
                    PromptGeneration::new(4),
                    "only the latest generation should receive Update"
                );
            }
            other => return Err(format!("expected Update, got {other:?}").into()),
        }

        let extra = tokio::time::timeout(Duration::from_millis(300), reader.read_message()).await;
        assert!(
            extra.is_err(),
            "stale generation Updates should be suppressed"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_env_dependency_prevents_cache_reuse()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = Config {
            module: vec![ModuleDef {
                name: "env-dep".to_owned(),
                when: ModuleWhen {
                    files: vec![],
                    env: vec!["MY_VAR".to_owned()],
                },
                source: vec![SourceDef {
                    name: "value".to_owned(),
                    env: None,
                    file: None,
                    command: Some(vec![
                        "sh".to_owned(),
                        "-c".to_owned(),
                        "echo dynamic".to_owned(),
                    ]),
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: Some("via".to_owned()),
                arbitration: None,
            }],
            ..Config::default()
        };
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            call_count: Some(Arc::clone(&call_count)),
            ..MockGitProvider::default()
        };
        let harness = TestHarness::start_with_config(provider, config).await?;
        let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
        let (mut reader, mut writer) = harness.connect().await?;

        let mut req1 = make_request(&cwd, 1, 80);
        req1.env_vars = vec![("MY_VAR".to_owned(), "a".to_owned())];
        writer.write_message(&Message::Request(req1)).await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let mut req2 = make_request(&cwd, 2, 80);
        req2.env_vars = vec![("MY_VAR".to_owned(), "b".to_owned())];
        writer.write_message(&Message::Request(req2)).await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "different env values should produce different cache keys"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_caches_slow_results_with_file_dependency()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = Config {
            module: vec![make_sleep_module("file-dep", 50, "CACHED")],
            ..Config::default()
        };
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            call_count: Some(Arc::clone(&call_count)),
            ..MockGitProvider::default()
        };
        let harness = TestHarness::start_with_config(provider, config).await?;
        let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
        let (mut reader, mut writer) = harness.connect().await?;

        writer
            .write_message(&Message::Request(make_request(&cwd, 1, 80)))
            .await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        writer
            .write_message(&Message::Request(make_request(&cwd, 2, 80)))
            .await?;
        let rr = reader.read_message().await?;
        match &rr {
            Some(Message::RenderResult(rr)) => {
                assert!(
                    rr.left1.contains("main"),
                    "cache hit should include git branch: {}",
                    rr.left1
                );
            }
            other => {
                return Err(format!("expected RenderResult with cache hit, got {other:?}").into());
            }
        }
        let update = tokio::time::timeout(Duration::from_millis(200), reader.read_message()).await;
        assert!(
            update.is_err(),
            "file-dep module with cache hit should not trigger Update"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "cache hit should still revalidate git in background"
        );

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_status_request_returns_metrics() -> Result<(), Box<dyn std::error::Error>>
    {
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            ..MockGitProvider::default()
        };
        let harness = TestHarness::start(provider).await?;
        let (mut reader, mut writer) = harness.connect().await?;

        // Send a prompt request first so metrics have data
        writer
            .write_message(&Message::Request(make_request("/tmp", 1, 80)))
            .await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;

        // Send StatusRequest
        writer
            .write_message(&Message::StatusRequest(capsule_protocol::StatusRequest {
                version: capsule_protocol::PROTOCOL_VERSION,
            }))
            .await?;

        let resp = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        match resp {
            Some(Message::StatusResponse(s)) => {
                assert!(s.pid > 0, "pid should be positive");
                assert!(
                    s.requests_total >= 1,
                    "should have at least one request: {}",
                    s.requests_total
                );
                assert!(
                    s.connections_active >= 1,
                    "should have at least one active connection"
                );
            }
            other => return Err(format!("expected StatusResponse, got {other:?}").into()),
        }

        harness.shutdown().await
    }

    #[tokio::test]
    async fn test_daemon_cache_off_always_recomputes_slow_modules()
    -> Result<(), Box<dyn std::error::Error>> {
        let call_count = count_git_calls();
        let provider = MockGitProvider {
            status: Some(GitStatus {
                branch: Some("main".to_owned()),
                ..GitStatus::default()
            }),
            call_count: Some(Arc::clone(&call_count)),
            ..MockGitProvider::default()
        };
        let config = Config {
            cache: CacheConfig {
                slow: SlowCacheMode::Off,
            },
            ..Config::default()
        };
        let harness = TestHarness::start_with_config(provider, config).await?;
        let cwd = harness.cwd_str().ok_or("missing work dir")?.to_owned();
        let (mut reader, mut writer) = harness.connect().await?;

        // First request: cache miss, slow compute runs git.
        writer
            .write_message(&Message::Request(make_request(&cwd, 1, 80)))
            .await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second request: with cache off, should recompute (not serve from cache).
        writer
            .write_message(&Message::Request(make_request(&cwd, 2, 80)))
            .await?;
        let _ = reader.read_message().await?;
        let _ = tokio::time::timeout(Duration::from_secs(5), reader.read_message()).await??;
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "cache off should recompute slow modules every request"
        );

        harness.shutdown().await
    }
}
