use std::{os::unix::fs::MetadataExt as _, path::PathBuf, sync::Arc};

use capsule_protocol::BuildId;
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Mutex,
};

use super::{DaemonError, INODE_CHECK_INTERVAL, ReloadableConfig, SharedState, request};
use crate::module::GitProvider;

/// Shared state for the accept loops (avoids passing many args).
pub(super) struct AcceptCtx<G> {
    pub(super) home_dir: Arc<PathBuf>,
    pub(super) git_provider: G,
    pub(super) build_id: Arc<Option<BuildId>>,
    pub(super) state: Arc<Mutex<SharedState>>,
    pub(super) config: Arc<Mutex<ReloadableConfig>>,
}

impl<G> AcceptCtx<G> {
    fn spawn_handler(&self, stream: UnixStream)
    where
        G: GitProvider + Clone + Send + 'static,
    {
        let ctx = request::ConnectionCtx {
            state: Arc::clone(&self.state),
            home_dir: Arc::clone(&self.home_dir),
            git_provider: self.git_provider.clone(),
            build_id: Arc::clone(&self.build_id),
            config: Arc::clone(&self.config),
        };
        tokio::spawn(async move {
            if let Err(e) = request::handle_connection(stream, ctx).await {
                tracing::warn!(error = %e, "client connection error");
            }
        });
    }
}

/// Accept loop with inode monitoring (standalone/bound mode).
pub(super) async fn run_bound<G: GitProvider + Clone + Send + Sync + 'static>(
    socket_path: PathBuf,
    listener: UnixListener,
    shutdown: impl std::future::Future<Output = ()>,
    ctx: AcceptCtx<G>,
) -> Result<(), DaemonError> {
    let original_inode = std::fs::metadata(&socket_path)?.ino();
    let socket_path = Arc::new(socket_path);

    tokio::pin!(shutdown);

    let mut inode_check = tokio::time::interval(INODE_CHECK_INTERVAL);
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
        let path = Arc::clone(&socket_path);
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(&*path)).await;
    }
    Ok(())
}

/// Accept loop without inode monitoring (socket activation mode).
pub(super) async fn run_activated<G: GitProvider + Clone + Send + Sync + 'static>(
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

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use capsule_protocol::BuildId;
    use tokio::net::UnixStream;

    use super::super::test_support::{MockGitProvider, TestHarness};
    use crate::daemon::{
        ConfigSource, Server,
        listener::{self, ListenerMode},
    };

    #[tokio::test]
    async fn test_daemon_stale_socket_cleanup() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("test.sock");

        {
            let _listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
        }
        assert!(socket_path.exists(), "stale socket file should exist");

        let home = dir.path().join("home");
        std::fs::create_dir_all(&home)?;

        let listener =
            listener::acquire_listener(&listener::ListenerSource::Bind(socket_path.clone()))?;
        let server = Server::new(
            home,
            MockGitProvider { status: None },
            Some(BuildId::new("test-build-id".to_owned())),
            ListenerMode::Bound(socket_path.clone()),
            ConfigSource::new(Arc::new(crate::config::Config::default()), None),
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
            Some(BuildId::new("test-build-id".to_owned())),
            ListenerMode::Bound(socket_path.clone()),
            ConfigSource::new(Arc::new(crate::config::Config::default()), None),
        );

        let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_handle = tokio::spawn(async move {
            server
                .run(listener, async {
                    let _ = shutdown_rx.await;
                })
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::remove_file(&socket_path)?;

        let result = tokio::time::timeout(Duration::from_secs(2), server_handle).await??;
        assert!(
            result.is_ok(),
            "server should shut down cleanly: {result:?}"
        );
        Ok(())
    }
}
