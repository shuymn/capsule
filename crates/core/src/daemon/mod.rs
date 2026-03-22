//! Daemon server — listens on a Unix socket and serves prompt requests.
//!
//! The daemon runs the request pipeline: fast modules → cache check → respond →
//! slow module recomputation → update. Sessions and a bounded cache track
//! per-client generation and slow module results.

mod accept;
mod cache;
pub mod listener;
mod prompt;
mod request;
mod session;
mod stats;

#[cfg(test)]
mod parallel_tests;
#[cfg(test)]
mod test_support;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use cache::BoundedCache;
use capsule_protocol::{BuildId, ConfigGeneration, DepHash};
use listener::ListenerMode;
use session::SessionMap;
use tokio::{
    net::UnixListener,
    sync::{Mutex, watch},
};

use crate::{
    config::{Config, ConfigLoadError},
    module::{GitProvider, ResolvedModule, resolve_modules},
};

const CACHE_MAX_SIZE: usize = 1024;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CacheKey {
    cwd: String,
    config_generation: ConfigGeneration,
    dep_hash: DepHash,
}

impl CacheKey {
    const fn new(cwd: String, config_generation: ConfigGeneration, dep_hash: DepHash) -> Self {
        Self {
            cwd,
            config_generation,
            dep_hash,
        }
    }
}

pub(super) struct SharedState {
    sessions: SessionMap,
    cache: BoundedCache<CacheKey, Arc<prompt::SlowOutput>>,
    inflight: HashMap<CacheKey, watch::Sender<Option<Arc<prompt::SlowOutput>>>>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            sessions: SessionMap::new(),
            cache: BoundedCache::new(CACHE_MAX_SIZE),
            inflight: HashMap::new(),
        }
    }

    pub(super) fn cache_len(&self) -> usize {
        self.cache.len()
    }

    pub(super) fn session_len(&self) -> usize {
        self.sessions.len()
    }
}

pub(super) struct ReloadableConfig {
    path: Option<PathBuf>,
    modified_at: Option<SystemTime>,
    generation: ConfigGeneration,
    config: Arc<Config>,
    modules: Arc<Vec<ResolvedModule>>,
}

impl ReloadableConfig {
    pub(super) const fn generation(&self) -> ConfigGeneration {
        self.generation
    }

    fn new(config: Arc<Config>, path: Option<PathBuf>) -> Self {
        let modules = Arc::new(resolve_modules(&config.module));
        let modified_at = path
            .as_ref()
            .and_then(|config_path| std::fs::metadata(config_path).ok())
            .and_then(|metadata| metadata.modified().ok());
        Self {
            path,
            modified_at,
            generation: ConfigGeneration::new(0),
            config,
            modules,
        }
    }

    async fn snapshot(
        &mut self,
        state: &Arc<Mutex<SharedState>>,
        metrics: &stats::DaemonStats,
    ) -> (Arc<Config>, Arc<Vec<ResolvedModule>>, ConfigGeneration) {
        if let Err(error) = self.reload_if_needed(state, metrics).await {
            metrics
                .config_reload_errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(error = %error, "config hot-reload check failed");
        }
        (
            Arc::clone(&self.config),
            Arc::clone(&self.modules),
            self.generation,
        )
    }

    async fn reload_if_needed(
        &mut self,
        state: &Arc<Mutex<SharedState>>,
        metrics: &stats::DaemonStats,
    ) -> Result<(), ConfigLoadError> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };

        let observed_mtime =
            load_modified_time(&path)
                .await
                .map_err(|source| ConfigLoadError::Read {
                    path: path.clone(),
                    source,
                })?;
        if observed_mtime == self.modified_at {
            return Ok(());
        }

        match read_config_async(&path).await? {
            Some(config) => {
                tracing::debug!(path = %path.display(), "reloaded config");
                self.config = Arc::new(config);
                self.modules = Arc::new(resolve_modules(&self.config.module));
                self.modified_at = observed_mtime;
                self.generation = ConfigGeneration::new(self.generation.get().saturating_add(1));
                metrics
                    .config_reloads
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mut shared = state.lock().await;
                shared.cache.clear();
            }
            None => {
                self.modified_at = None;
            }
        }

        Ok(())
    }
}

async fn load_modified_time(path: &Path) -> std::io::Result<Option<SystemTime>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || match std::fs::metadata(&path) {
        Ok(metadata) => metadata.modified().map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    })
    .await
    .map_err(|error| std::io::Error::other(format!("join error: {error}")))?
}

async fn read_config_async(path: &Path) -> Result<Option<Config>, ConfigLoadError> {
    let path = path.to_path_buf();
    let path_for_read = path.clone();
    tokio::task::spawn_blocking(move || crate::config::read_config(&path_for_read))
        .await
        .map_err(|source| ConfigLoadError::Read {
            path: path.clone(),
            source: std::io::Error::other(format!("join error: {source}")),
        })?
}

/// Daemon server that listens on a Unix domain socket.
///
/// Generic over `G` to allow injecting a mock [`GitProvider`] in tests.
pub struct Server<G> {
    home_dir: PathBuf,
    git_provider: G,
    build_id: Option<BuildId>,
    listener_mode: ListenerMode,
    config_source: ConfigSource,
}

pub struct ConfigSource {
    config: Arc<Config>,
    path: Option<PathBuf>,
}

impl ConfigSource {
    #[must_use]
    pub const fn new(config: Arc<Config>, path: Option<PathBuf>) -> Self {
        Self { config, path }
    }
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
    /// * `config_source` — initial prompt configuration and its source path
    pub const fn new(
        home_dir: PathBuf,
        git_provider: G,
        build_id: Option<BuildId>,
        listener_mode: ListenerMode,
        config_source: ConfigSource,
    ) -> Self {
        Self {
            home_dir,
            git_provider,
            build_id,
            listener_mode,
            config_source,
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

        let ctx = accept::AcceptCtx {
            home_dir: Arc::new(self.home_dir),
            git_provider: self.git_provider,
            build_id: Arc::new(self.build_id),
            state: Arc::new(Mutex::new(SharedState::new())),
            config: Arc::new(Mutex::new(ReloadableConfig::new(
                self.config_source.config,
                self.config_source.path,
            ))),
            stats: Arc::new(stats::DaemonStats::new()),
        };

        match self.listener_mode {
            ListenerMode::Bound(socket_path) => {
                accept::run_bound(socket_path, listener, shutdown, ctx).await
            }
            ListenerMode::Activated => accept::run_activated(listener, shutdown, ctx).await,
        }
    }
}
