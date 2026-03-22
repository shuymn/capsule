use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use capsule_protocol::{
    BuildId, MessageReader, MessageWriter, PROTOCOL_VERSION, PromptGeneration, Request, SessionId,
};
use tokio::net::UnixStream;

use super::{
    ConfigSource, DaemonError, Server,
    listener::{self, ListenerMode},
};
use crate::{
    config::{Config, ModuleDef, ModuleWhen, SourceDef, StyleConfig},
    module::{GitError, GitProvider, GitStatus},
    sealed,
};

#[derive(Debug, Clone, Default)]
pub(super) struct MockGitProvider {
    pub(super) status: Option<GitStatus>,
    pub(super) delay: Duration,
    pub(super) call_count: Option<Arc<AtomicUsize>>,
}

impl sealed::Sealed for MockGitProvider {}

impl GitProvider for MockGitProvider {
    fn status(&self, _cwd: &Path, _path_env: Option<&str>) -> Result<Option<GitStatus>, GitError> {
        if let Some(call_count) = &self.call_count {
            call_count.fetch_add(1, Ordering::SeqCst);
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
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
        generation: PromptGeneration::new(generation),
        cwd: cwd.to_owned(),
        cols,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".to_owned(),
        env_vars: vec![],
    }
}

/// Create a slow module definition that sleeps for `sleep_ms` milliseconds
/// then outputs `output`. Requires a "marker" file in cwd to trigger.
pub(super) fn make_sleep_module(name: &str, sleep_ms: u32, output: &str) -> ModuleDef {
    let sleep_secs = f64::from(sleep_ms) / 1000.0;
    ModuleDef {
        name: name.to_owned(),
        when: ModuleWhen {
            files: vec!["marker".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            env: None,
            file: None,
            command: Some(vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!("sleep {sleep_secs}; echo {output}"),
            ]),
            regex: None,
        }],
        format: "{value}".to_owned(),
        icon: None,
        style: StyleConfig::default(),
        connector: Some("via".to_owned()),
        arbitration: None,
    }
}

pub(super) struct TestHarness {
    pub(super) socket_path: PathBuf,
    pub(super) work_dir: Option<PathBuf>,
    _dir: tempfile::TempDir,
    _work_dir: Option<tempfile::TempDir>,
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
        Self::start_impl(provider, build_id, Config::default(), false).await
    }

    /// Start a daemon with a custom config and a work directory containing a
    /// "marker" file (needed for modules with `when.files = ["marker"]`).
    pub(super) async fn start_with_config(
        provider: MockGitProvider,
        config: Config,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_impl(
            provider,
            Some(BuildId::new("test-build-id".to_owned())),
            config,
            true,
        )
        .await
    }

    pub(super) async fn start_with_config_path(
        provider: MockGitProvider,
        config: Config,
        config_path: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_impl_with_config_path(
            provider,
            Some(BuildId::new("test-build-id".to_owned())),
            config,
            true,
            Some(config_path),
        )
        .await
    }

    async fn start_impl(
        provider: MockGitProvider,
        build_id: Option<BuildId>,
        config: Config,
        create_work_dir: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_impl_with_config_path(provider, build_id, config, create_work_dir, None).await
    }

    async fn start_impl_with_config_path(
        provider: MockGitProvider,
        build_id: Option<BuildId>,
        config: Config,
        create_work_dir: bool,
        config_path: Option<PathBuf>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("test.sock");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home)?;

        let (work_dir_td, work_dir_path) = if create_work_dir {
            let wd = tempfile::tempdir()?;
            std::fs::write(wd.path().join("marker"), "")?;
            let path = wd.path().to_path_buf();
            (Some(wd), Some(path))
        } else {
            (None, None)
        };

        let listener =
            listener::acquire_listener(&listener::ListenerSource::Bind(socket_path.clone()))?;
        let server = Server::new(
            home,
            provider,
            build_id,
            ListenerMode::Bound(socket_path.clone()),
            ConfigSource::new(Arc::new(config), config_path),
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
            work_dir: work_dir_path,
            _dir: dir,
            _work_dir: work_dir_td,
            shutdown_tx: Some(shutdown_tx),
            server_handle,
        })
    }

    /// Returns the work directory path as a string, if available.
    pub(super) fn cwd_str(&self) -> Option<&str> {
        self.work_dir.as_ref().and_then(|p| p.to_str())
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
