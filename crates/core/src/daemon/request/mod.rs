use std::{path::PathBuf, sync::Arc, time::Duration};

use capsule_protocol::{
    BuildId, ConfigGeneration, DepHash, HelloAck, Message, MessageReader, MessageWriter,
    PromptGeneration, RenderResult, Request, Update,
};
use tokio::{
    net::UnixStream,
    sync::{Mutex, watch},
    task::JoinSet,
};

use super::{
    CacheKey, DaemonError, ReloadableConfig, SESSION_TTL, SharedState, SlowWorkClaim, prompt,
    session::Session, stats::DaemonStats,
};
use crate::module::{
    CustomModuleInfo, GitProvider, ModuleSpeed, RequestFacts, ResolvedModule, ResolvedSource,
    git::render_status_with_styles, required_env_var_names,
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
    pub(super) worker_tasks: Arc<Mutex<JoinSet<()>>>,
}

struct RequestCtx<G> {
    state: Arc<Mutex<SharedState>>,
    writer: Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    home_dir: Arc<PathBuf>,
    git_provider: G,
    config: Arc<Mutex<ReloadableConfig>>,
    stats: Arc<DaemonStats>,
    worker_tasks: Arc<Mutex<JoinSet<()>>>,
}

async fn write_message(
    writer: &Arc<Mutex<MessageWriter<tokio::net::unix::OwnedWriteHalf>>>,
    message: &Message,
) -> Result<(), DaemonError> {
    writer.lock().await.write_message(message).await?;
    Ok(())
}

pub(super) async fn handle_connection<G: GitProvider + Clone + Send + 'static>(
    stream: UnixStream,
    ctx: ConnectionCtx<G>,
) -> Result<(), DaemonError> {
    let (reader, writer) = stream.into_split();
    let mut msg_reader = MessageReader::new(reader);
    let msg_writer = Arc::new(Mutex::new(MessageWriter::new(writer)));
    let mut connection_tasks = JoinSet::new();

    loop {
        tokio::select! {
            message = msg_reader.read_message() => {
                match message {
                    Ok(Some(Message::Request(req))) => {
                        let req_ctx = RequestCtx {
                            state: Arc::clone(&ctx.state),
                            writer: Arc::clone(&msg_writer),
                            home_dir: Arc::clone(&ctx.home_dir),
                            git_provider: ctx.git_provider.clone(),
                            config: Arc::clone(&ctx.config),
                            stats: Arc::clone(&ctx.stats),
                            worker_tasks: Arc::clone(&ctx.worker_tasks),
                        };
                        handle_request(req, req_ctx, &mut connection_tasks).await?;
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
                            let (_, modules, _) = config.snapshot(&ctx.stats).await;
                            drop(config);
                            modules
                        };
                        let ack = HelloAck {
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
            Some(joined) = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                if let Err(error) = joined {
                    tracing::debug!(error = %error, "connection task failed");
                }
            }
        }
    }

    connection_tasks.abort_all();
    while connection_tasks.join_next().await.is_some() {}

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

    let git_future = if config.git.disabled {
        None
    } else {
        let git_cwd = facts.cwd().to_path_buf();
        let git_styles = crate::module::git::GitStyles {
            branch: config.git.prompt_style(),
            detached_hash: config.git.detached_hash_prompt_style(),
            indicator: config.git.indicator_prompt_style(),
            state: config.git.state_prompt_style(),
            color_map: config.color_map,
        };
        let git_path_env = facts.command_path_env().map(ToOwned::to_owned);
        Some(async move {
            match git_provider.status_async(git_cwd, git_path_env).await {
                Ok(Some(status)) => {
                    render_status_with_styles(&status, &git_styles).map(|output| output.content)
                }
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(error = %error, "git status failed");
                    None
                }
            }
        })
    };

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
            match git_future {
                Some(future) => tokio::time::timeout_at(deadline, future)
                    .await
                    .unwrap_or_else(|_| {
                        daemon_stats.git_timeouts.fetch_add(1, Ordering::Relaxed);
                        None
                    }),
                None => None,
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

/// Spawn a background task that computes slow modules, refreshes the cache when
/// the output changed, and wakes all waiters for the cache key.
#[expect(
    clippy::too_many_arguments,
    reason = "all parameters are owned types forwarded into a spawned task"
)]
async fn spawn_slow_worker<G: GitProvider + Send + 'static>(
    cached: Option<Arc<prompt::SlowOutput>>,
    git_provider: G,
    facts: Arc<RequestFacts>,
    modules: Arc<Vec<ResolvedModule>>,
    config: Arc<crate::config::Config>,
    shared_state: Arc<Mutex<SharedState>>,
    worker_tasks: Arc<Mutex<JoinSet<()>>>,
    daemon_stats: Arc<DaemonStats>,
    cache_key: CacheKey,
    cache_enabled: bool,
) {
    use std::sync::atomic::Ordering;

    daemon_stats
        .slow_computes_started
        .fetch_add(1, Ordering::Relaxed);

    let mut tasks = worker_tasks.lock().await;
    while let Some(joined) = tasks.try_join_next() {
        if let Err(error) = joined {
            tracing::debug!(error = %error, "slow worker task failed");
        }
    }
    tasks.spawn(async move {
        let updated_slow =
            compute_slow_modules(git_provider, &facts, &modules, &config, &daemon_stats).await;

        let changed = cached
            .as_ref()
            .is_none_or(|cached| updated_slow != **cached);

        let updated_slow = Arc::new(updated_slow);
        let sender = {
            let mut state_locked = shared_state.lock().await;
            let sender = state_locked.inflight.remove(&cache_key);
            if changed
                && cache_enabled
                && state_locked
                    .cache
                    .insert(cache_key, Arc::clone(&updated_slow))
            {
                daemon_stats.cache_evictions.fetch_add(1, Ordering::Relaxed);
            }
            drop(state_locked);
            sender
        };

        if changed && let Some(sender) = sender {
            let _ = sender.send(Some(updated_slow));
        }
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
    let mut results: Vec<(&ResolvedModule, Option<CustomModuleInfo>)> = input
        .facts
        .matching_modules(input.modules, input.speed)
        .map(|def| (def, None))
        .collect();

    if results.is_empty() {
        return Vec::new();
    }
    let mut join_set = JoinSet::new();

    let mut deferred = Vec::new();
    for (idx, (def, detected)) in results.iter_mut().enumerate() {
        if should_detect_inline(input.speed, def) {
            *detected = input.facts.detect_module(def).await;
        } else {
            deferred.push((idx, (*def).clone()));
        }
    }

    if !deferred.is_empty() {
        for (idx, def) in deferred {
            let facts = Arc::clone(&input.facts);
            join_set.spawn(async move { (idx, facts.detect_module(&def).await) });
        }

        let deadline = tokio::time::Instant::now() + input.timeout;

        while !join_set.is_empty() {
            match tokio::time::timeout_at(deadline, join_set.join_next()).await {
                Ok(Some(Ok((idx, info)))) => {
                    results[idx].1 = info;
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

    RequestFacts::arbitrate_detected_slots(results)
}

async fn handle_request<G: GitProvider + Send + 'static>(
    req: Request,
    ctx: RequestCtx<G>,
    connection_tasks: &mut JoinSet<()>,
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
    } = req;

    let config_snap = {
        let mut reloadable = ctx.config.lock().await;
        let snapshot = reloadable.snapshot(&ctx.stats).await;
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
    let slow_claim = ctx
        .state
        .lock()
        .await
        .claim_slow_work(&collected.cache_key, cache_enabled);
    match &slow_claim {
        SlowWorkClaim::Cached { should_start, .. } => {
            ctx.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            if !should_start {
                ctx.stats.inflight_coalesces.fetch_add(1, Ordering::Relaxed);
            }
        }
        SlowWorkClaim::Pending { should_start, .. } => {
            ctx.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
            if !should_start {
                ctx.stats.inflight_coalesces.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let cached_slow = match &slow_claim {
        SlowWorkClaim::Cached { slow, .. } => Some(Arc::clone(slow)),
        SlowWorkClaim::Pending { .. } => None,
    };

    let lines = prompt::compose_prompt(
        &fast,
        cached_slow.as_deref(),
        usize::from(gated.cols),
        &config_snap.config,
    );

    let sent_left1 = lines.left1.clone();
    let sent_left2 = lines.left2.clone();
    let slow_config = Arc::clone(&config_snap.config);
    let slow_modules = Arc::clone(&config_snap.modules);
    let CollectedFacts { facts, cache_key } = collected;
    let state = Arc::clone(&ctx.state);
    let writer = Arc::clone(&ctx.writer);
    let (receiver, should_start_compute, cached_for_worker) = match slow_claim {
        SlowWorkClaim::Cached {
            slow,
            receiver,
            should_start,
        } => (receiver, should_start, Some(slow)),
        SlowWorkClaim::Pending {
            receiver,
            should_start,
        } => (receiver, should_start, None),
    };

    if should_start_compute {
        spawn_slow_worker(
            cached_for_worker,
            ctx.git_provider,
            facts,
            slow_modules,
            Arc::clone(&slow_config),
            Arc::clone(&state),
            Arc::clone(&ctx.worker_tasks),
            Arc::clone(&ctx.stats),
            cache_key,
            cache_enabled,
        )
        .await;
    }

    let result = RenderResult {
        session_id: gated.session_id,
        generation: gated.generation,
        left1: sent_left1.clone(),
        left2: sent_left2.clone(),
        meta: lines.char_meta.clone(),
    };
    tracing::debug!(
        session_id = %gated.session_id,
        generation = gated.generation.get(),
        cwd = %gated.cwd,
        "sending RenderResult"
    );
    write_message(&writer, &Message::RenderResult(result)).await?;

    connection_tasks.spawn(wait_for_slow_update(SlowUpdateTarget {
        state: Arc::clone(&state),
        writer: Arc::clone(&writer),
        receiver,
        session_id: gated.session_id,
        generation: gated.generation,
        sent_left1,
        sent_left2,
        fast: fast.clone(),
        cols: gated.cols,
        config: Arc::clone(&slow_config),
    }));

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
mod tests;
