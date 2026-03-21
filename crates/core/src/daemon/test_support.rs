use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use capsule_protocol::{
    BuildId, MessageReader, MessageWriter, PROTOCOL_VERSION, Request, SessionId,
};
use tokio::net::UnixStream;

use super::{
    DaemonError, Server,
    listener::{self, ListenerMode},
};
use crate::{
    config::Config,
    module::{GitError, GitProvider, GitStatus},
};

#[derive(Debug, Clone)]
pub(super) struct MockGitProvider {
    pub(super) status: Option<GitStatus>,
}

impl GitProvider for MockGitProvider {
    fn status(&self, _cwd: &Path, _path_env: Option<&str>) -> Result<Option<GitStatus>, GitError> {
        Ok(self.status.clone())
    }
}

pub(super) fn test_sid() -> SessionId {
    SessionId::from_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
}

pub(super) fn make_request(cwd: &str, generation: u64, cols: u16) -> Request {
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

pub(super) struct TestHarness {
    pub(super) socket_path: PathBuf,
    _dir: tempfile::TempDir,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    server_handle: tokio::task::JoinHandle<Result<(), DaemonError>>,
}

impl TestHarness {
    pub(super) async fn start(
        provider: MockGitProvider,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_build_id(provider, Some(BuildId::new("test-build-id".to_owned()))).await
    }

    pub(super) async fn start_with_build_id(
        provider: MockGitProvider,
        build_id: Option<BuildId>,
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
            build_id,
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

        Ok(Self {
            socket_path,
            _dir: dir,
            shutdown_tx: Some(shutdown_tx),
            server_handle,
        })
    }

    pub(super) async fn connect(
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

    pub(super) async fn shutdown(mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.server_handle.await??;
        Ok(())
    }
}
