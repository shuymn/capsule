//! Bounded acquisition from a shell snapshot, with owned subprocess lifetimes.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use capsule_protocol::session::Snapshot;
use rustix::{
    fs::{Mode, OFlags},
    process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, ChildStdout, Command},
    signal::unix::{Signal as ChildSignals, SignalKind, signal},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::{JoinError, JoinSet},
    time::{Instant, sleep_until, timeout},
};
use tokio_util::sync::CancellationToken;

const ACQUISITION_TIMEOUT: Duration = Duration::from_millis(500);
const CLEANUP_TIMEOUT: Duration = Duration::from_millis(100);
/// Maximum bytes retained for one command, file, or exported environment value.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// An acquisition failure. Errors do not contain captured environment values.
#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    /// The command has no executable.
    #[error("empty command")]
    InvalidCommand,
    /// The owning execution generation was cancelled.
    #[error("acquisition cancelled")]
    Cancelled,
    /// Waiting for capacity, acquiring, or collecting a process exceeded its deadline.
    #[error("acquisition timed out")]
    TimedOut,
    /// A file or command exceeded the fixed byte budget.
    #[error("acquisition exceeds {MAX_OUTPUT_BYTES} bytes")]
    OutputTooLarge,
    /// The command ran but reported failure.
    #[error("command exited with {0}")]
    FailedExit(ExitStatus),
    /// An OS operation failed.
    #[error("acquisition I/O: {0}")]
    Io(#[from] io::Error),
    /// A bounded blocking operation could not finish normally.
    #[error("acquisition task: {0}")]
    Task(#[from] JoinError),
}

/// Shared acquisition limits: four subprocesses and two blocking filesystem jobs.
///
/// Each operation has a 500 ms deadline including capacity waiting. A cancelled
/// blocking job retains its capacity until it actually finishes. Process cleanup
/// gets another 100 ms; unusually slow reaping retains its process capacity in a
/// tracked cleanup task. Clone this runner to share those limits across modules.
#[derive(Clone)]
pub struct Runner {
    processes: Arc<Semaphore>,
    files: Arc<Semaphore>,
    reapers: Arc<Mutex<JoinSet<()>>>,
}

impl Runner {
    /// Capture bounded stdout using exactly the supplied cwd and environment.
    ///
    /// Stdin reads EOF and stderr is discarded. Each command owns a process
    /// group. Cancellation, timeout, excessive output, and normal completion all
    /// terminate remaining members before releasing the command's capacity.
    ///
    /// # Errors
    /// Returns cancellation, deadline, byte-limit, exit-status or OS failures.
    /// A descendant that explicitly leaves the process group is not contained.
    pub async fn command(
        &self,
        argv: &[String],
        snapshot: &Snapshot,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, AcquireError> {
        let (executable, arguments) = argv.split_first().ok_or(AcquireError::InvalidCommand)?;
        let deadline = Instant::now() + ACQUISITION_TIMEOUT;
        let permit = acquire_permit(self.processes.clone(), cancel, deadline).await?;
        // Register before spawning so even an immediate exit cannot lose its wakeup.
        let child_signals = signal(SignalKind::child())?;
        let child = Command::new(executable)
            .args(arguments)
            .current_dir(&snapshot.cwd)
            .env_clear()
            .envs(snapshot.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let mut command = OwnedCommand::new(child, permit, self.reapers.clone(), child_signals)?;
        let result = command.capture(cancel, deadline).await;
        let cleanup = command.finish(result.is_ok()).await;
        drop(command);
        // Preserve the acquisition failure; cleanup still ran and owns any reaper.
        let output = result?;
        let status = cleanup?;
        if status.success() {
            Ok(output)
        } else {
            Err(AcquireError::FailedExit(status))
        }
    }

    /// Read at most 64 KiB from a regular file without blocking the async runtime.
    ///
    /// # Errors
    /// Returns cancellation, deadline, size-limit or OS failures. Missing paths
    /// return `None`; non-regular files are rejected without waiting for writers.
    pub async fn file(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, AcquireError> {
        let path = path.to_owned();
        self.blocking(cancel, move || {
            let fd = match rustix::fs::open(
                &path,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Err(error) => return Err(io::Error::from(error).into()),
            };
            let file = File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "source is not a regular file",
                )
                .into());
            }
            let mut output = Vec::new();
            file.take(MAX_OUTPUT_BYTES as u64 + 1)
                .read_to_end(&mut output)?;
            check_size(output).map(Some)
        })
        .await
    }

    /// Check a path with the same bounded filesystem capacity as file acquisition.
    ///
    /// # Errors
    /// Returns cancellation, deadline or OS failures. Missing paths return false.
    pub async fn exists(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<bool, AcquireError> {
        self.metadata(path, cancel)
            .await
            .map(|metadata| metadata.is_some())
    }

    /// Read metadata with bounded blocking capacity and a cancellation deadline.
    ///
    /// # Errors
    /// Returns cancellation, deadline or OS failures. Missing paths return `None`.
    pub async fn metadata(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<Option<std::fs::Metadata>, AcquireError> {
        let path = path.to_owned();
        self.blocking(cancel, move || match std::fs::metadata(path) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        })
        .await
    }

    async fn blocking<T, F>(
        &self,
        cancel: &CancellationToken,
        operation: F,
    ) -> Result<T, AcquireError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, AcquireError> + Send + 'static,
    {
        let deadline = Instant::now() + ACQUISITION_TIMEOUT;
        let permit = acquire_permit(self.files.clone(), cancel, deadline).await?;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation()
        });
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(AcquireError::Cancelled),
            () = sleep_until(deadline) => Err(AcquireError::TimedOut),
            result = task => result?,
        }
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            processes: Arc::new(Semaphore::new(4)),
            files: Arc::new(Semaphore::new(2)),
            reapers: Arc::new(Mutex::new(JoinSet::new())),
        }
    }
}

struct OwnedCommand {
    child: Option<Child>,
    stdout: ChildStdout,
    child_signals: ChildSignals,
    group: Pid,
    group_owned: bool,
    permit: Option<OwnedSemaphorePermit>,
    reapers: Arc<Mutex<JoinSet<()>>>,
}

impl OwnedCommand {
    fn new(
        mut child: Child,
        permit: OwnedSemaphorePermit,
        reapers: Arc<Mutex<JoinSet<()>>>,
        child_signals: ChildSignals,
    ) -> Result<Self, AcquireError> {
        let group = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::other("missing child process id"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing child stdout"))?;
        Ok(Self {
            child: Some(child),
            stdout,
            child_signals,
            group,
            group_owned: true,
            permit: Some(permit),
            reapers,
        })
    }

    async fn capture(
        &mut self,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Vec<u8>, AcquireError> {
        let capture = async {
            // Observe exit without reaping: the leader's PID reserves the PGID
            // until group cleanup has signalled every remaining member.
            let (output, ()) = tokio::try_join!(
                read_output(&mut self.stdout),
                observe_exit(self.group, &mut self.child_signals)
            )?;
            Ok(output)
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(AcquireError::Cancelled),
            () = sleep_until(deadline) => Err(AcquireError::TimedOut),
            result = capture => result,
        }
    }

    async fn finish(&mut self, observed_exit_and_eof: bool) -> Result<ExitStatus, AcquireError> {
        let termination = terminate_group(self.group, observed_exit_and_eof);
        // From this point Child::wait may release the PID. Never signal it again.
        self.group_owned = false;
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| io::Error::other("child already reaped"))?;
        let cleanup = async {
            let mut discard = tokio::io::sink();
            let (status, _) = tokio::try_join!(
                child.wait(),
                tokio::io::copy(&mut self.stdout, &mut discard)
            )?;
            io::Result::Ok(status)
        };
        let status = timeout(CLEANUP_TIMEOUT, cleanup)
            .await
            .map_err(|_elapsed| AcquireError::TimedOut)??;
        drop(self.child.take());
        drop(self.permit.take());
        termination.map(|()| status)
    }
}

impl Drop for OwnedCommand {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if self.group_owned
            && let Err(error) = terminate_group(self.group, false)
        {
            tracing::warn!(%error, "command group cleanup failed");
        }
        let permit = self.permit.take();
        // Dropping a future also retains capacity until its killed child is reaped.
        // On runtime teardown Tokio's Child drop provides its final reap fallback.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut reapers = self
            .reapers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some(result) = reapers.try_join_next() {
            if let Err(error) = result {
                tracing::warn!(%error, "command cleanup task failed");
            }
        }
        reapers.spawn(async move {
            let _permit = permit;
            if let Err(error) = child.wait().await {
                tracing::warn!(%error, "command reap failed");
            }
        });
    }
}

async fn acquire_permit(
    pool: Arc<Semaphore>,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<OwnedSemaphorePermit, AcquireError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(AcquireError::Cancelled),
        () = sleep_until(deadline) => Err(AcquireError::TimedOut),
        permit = pool.acquire_owned() => permit.map_err(|error| io::Error::other(error).into()),
    }
}

async fn read_output(output: impl AsyncRead + Unpin) -> Result<Vec<u8>, AcquireError> {
    let mut bytes = Vec::new();
    output
        .take(MAX_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    check_size(bytes)
}

async fn observe_exit(pid: Pid, signals: &mut ChildSignals) -> Result<(), AcquireError> {
    loop {
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        if waitid(WaitId::Pid(pid), options)
            .map_err(io::Error::from)?
            .is_some()
        {
            return Ok(());
        }
        if signals.recv().await.is_none() {
            return Err(
                io::Error::new(io::ErrorKind::BrokenPipe, "child signal stream closed").into(),
            );
        }
    }
}

fn check_size(bytes: Vec<u8>) -> Result<Vec<u8>, AcquireError> {
    if bytes.len() > MAX_OUTPUT_BYTES {
        Err(AcquireError::OutputTooLarge)
    } else {
        Ok(bytes)
    }
}

fn terminate_group(group: Pid, observed_exit_and_eof: bool) -> Result<(), AcquireError> {
    match kill_process_group(group, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        // Darwin reports EPERM for a process group containing only zombies.
        // Require both non-reaping exit observation and closed stdout before
        // accepting that result; live same-user descendants remain signalable.
        Err(rustix::io::Errno::PERM) if cfg!(target_os = "macos") && observed_exit_and_eof => {
            Ok(())
        }
        Err(error) => Err(io::Error::from(error).into()),
    }
}

#[cfg(test)]
mod tests;
