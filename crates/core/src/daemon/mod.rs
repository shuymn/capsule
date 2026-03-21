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

#[cfg(test)]
mod test_support;

use std::{path::PathBuf, sync::Arc, time::Duration};

use cache::BoundedCache;
use capsule_protocol::BuildId;
use listener::ListenerMode;
use session::SessionMap;
use tokio::{net::UnixListener, sync::Mutex};

use crate::{
    config::Config,
    module::{GitProvider, ResolvedModule, resolve_modules},
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

pub(super) struct SharedState {
    sessions: SessionMap,
    cache: BoundedCache<prompt::SlowOutput>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            sessions: SessionMap::new(),
            cache: BoundedCache::new(CACHE_MAX_SIZE, CACHE_TTL),
        }
    }
}

/// Daemon server that listens on a Unix domain socket.
///
/// Generic over `G` to allow injecting a mock [`GitProvider`] in tests.
pub struct Server<G> {
    home_dir: PathBuf,
    git_provider: G,
    build_id: Option<BuildId>,
    listener_mode: ListenerMode,
    config: Arc<Config>,
    modules: Arc<Vec<ResolvedModule>>,
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
        build_id: Option<BuildId>,
        listener_mode: ListenerMode,
        config: Arc<Config>,
    ) -> Self {
        let modules = Arc::new(resolve_modules(&config.module));
        Self {
            home_dir,
            git_provider,
            build_id,
            listener_mode,
            config,
            modules,
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
            config: self.config,
            modules: self.modules,
        };

        match self.listener_mode {
            ListenerMode::Bound(socket_path) => {
                accept::run_bound(socket_path, listener, shutdown, ctx).await
            }
            ListenerMode::Activated => accept::run_activated(listener, shutdown, ctx).await,
        }
    }
}
