use std::{
    error::Error,
    ffi::OsString,
    fs::File,
    os::unix::ffi::OsStringExt,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use tokio::{runtime::Runtime, sync::oneshot};

use super::*;
use capsule_protocol::session;

use crate::test_utils::mkfifo;

type TestResult = Result<(), Box<dyn Error>>;

fn runtime() -> io::Result<Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn snapshot(cwd: &Path) -> Result<Snapshot, session::Error> {
    with_env(
        cwd,
        std::env::var_os("PATH").map(|value| (OsString::from("PATH"), value)),
    )
}

fn with_env(
    cwd: &Path,
    env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<Snapshot, session::Error> {
    Snapshot::new(cwd.to_owned(), env.into_iter().collect())
}

/// A nonblocking reader of a `liveness` FIFO in `directory`, which the
/// commands `LIVE_DESCENDANT` starts hold open while they live.
fn liveness(directory: &Path) -> io::Result<File> {
    let fifo = directory.join("liveness");
    mkfifo(&fifo)?;
    open_nonblocking(&fifo)
}

/// A background descendant that writes `ready` and holds the FIFO open.
const LIVE_DESCENDANT: &str = "sh -c 'exec 3>liveness; printf ready >&3; sleep 60' &";

/// Read the FIFO until EOF, which proves every writer has exited. A leaked
/// descendant keeps it at `WouldBlock` until the deadline.
fn drain(reader: &mut File) -> io::Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut observed = Vec::new();
    loop {
        let mut chunk = [0; 64];
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(observed),
            Ok(count) => observed.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn argv(args: &[&str]) -> io::Result<Vec<String>> {
    let mut command = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    if let Some(program) = command.first_mut()
        && !Path::new(program.as_str()).is_absolute()
    {
        *program = crate::test_utils::executable(program)?;
    }
    Ok(command)
}

/// Covers: T06
#[test]
fn command_preserves_raw_environment_and_clears_unspecified_variables() -> TestResult {
    let directory = tempfile::tempdir()?;
    let input = with_env(
        directory.path(),
        [(
            OsString::from("CAPSULE_VALUE"),
            OsString::from_vec(b"raw\xff\n\t\\".to_vec()),
        )],
    )?;
    runtime()?.block_on(async {
        let output = Runner::default()
            .command(&argv(&["env", "-0"])?, &input, &CancellationToken::new())
            .await?;
        assert_eq!(output, b"CAPSULE_VALUE=raw\xff\n\t\\\0");
        Ok::<_, AcquireError>(())
    })?;
    Ok(())
}

#[test]
fn command_uses_snapshot_cwd_and_reports_failed_exit() -> TestResult {
    let directory = tempfile::tempdir()?;
    let expected = std::fs::canonicalize(directory.path())?;
    let input = snapshot(directory.path())?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let cancel = CancellationToken::new();
        let output = runner
            .command(&argv(&["pwd", "-P"])?, &input, &cancel)
            .await?;
        assert_eq!(output, format!("{}\n", expected.display()).as_bytes());
        let failure = runner
            .command(
                // Closing stdout early must not cut the wait for the exit status.
                &argv(&["sh", "-c", "printf hidden >&2; exec >&-; sleep 0.2; exit 7"])?,
                &input,
                &cancel,
            )
            .await;
        assert!(
            matches!(failure, Err(AcquireError::FailedExit(status)) if status.code() == Some(7))
        );
        Ok::<_, AcquireError>(())
    })?;
    Ok(())
}

/// Covers: T07
#[test]
fn command_rejects_excessive_stdout_and_releases_capacity() -> TestResult {
    let command = argv(&["sh", "-c", "printf '%65537s' x"])?;
    let input = snapshot(Path::new("/"))?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let output = runner
            .command(&command, &input, &CancellationToken::new())
            .await;
        assert!(matches!(output, Err(AcquireError::OutputTooLarge)));
        assert_eq!(runner.processes.available_permits(), 4);
    });
    Ok(())
}

/// Covers: T07
#[test]
fn timeout_stops_descendants_that_hold_stdout_after_the_leader_exits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut reader = liveness(directory.path())?;
    let command = argv(&["sh", "-c", &format!("{LIVE_DESCENDANT} exit 0")])?;
    let input = snapshot(directory.path())?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let result = timeout(
            Duration::from_secs(5),
            runner.command(&command, &input, &CancellationToken::new()),
        )
        .await;
        assert!(matches!(result, Ok(Err(AcquireError::TimedOut))));
        assert_eq!(runner.processes.available_permits(), 4);
    });
    assert_eq!(drain(&mut reader)?, b"ready");
    Ok(())
}

/// Covers: T07
#[test]
fn cancellation_stops_running_commands_and_prevents_queued_spawns() -> TestResult {
    let input = snapshot(Path::new("/"))?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let cancel = CancellationToken::new();
        let command_runner = runner.clone();
        let command_cancel = cancel.clone();
        let command_input = input.clone();
        let command = tokio::spawn(async move {
            command_runner
                .command(&argv(&["sleep", "60"])?, &command_input, &command_cancel)
                .await
        });
        timeout(Duration::from_secs(5), async {
            while runner.processes.available_permits() == 4 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        cancel.cancel();
        assert!(matches!(command.await?, Err(AcquireError::Cancelled)));
        assert_eq!(runner.processes.available_permits(), 4);
        let queued = runner
            .command(&argv(&["/does/not/exist"])?, &input, &cancel)
            .await;
        assert!(matches!(queued, Err(AcquireError::Cancelled)));
        Ok::<_, Box<dyn Error>>(())
    })?;
    Ok(())
}

/// Covers: T07
#[test]
fn dropped_command_future_stops_its_group_and_retains_its_slot_until_reaped() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut reader = liveness(directory.path())?;
    let args = argv(&["sh", "-c", &format!("{LIVE_DESCENDANT} sleep 60")])?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let command_runner = runner.clone();
        let input = snapshot(directory.path())?;
        let command = tokio::spawn(async move {
            command_runner
                .command(&args, &input, &CancellationToken::new())
                .await
        });
        // Wait until the descendant holds the FIFO, so EOF below means it exited.
        let mut ready = [0; 5];
        timeout(Duration::from_secs(5), async {
            while !matches!(reader.read(&mut ready), Ok(count) if count > 0) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        command.abort();
        assert!(command.await.is_err());
        timeout(Duration::from_secs(5), async {
            while runner.processes.available_permits() != 4 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        Ok::<_, Box<dyn Error>>(())
    })?;
    drain(&mut reader)?;
    Ok(())
}

/// Covers: T05, T07
#[test]
fn file_distinguishes_missing_empty_oversized_and_non_regular_sources() -> TestResult {
    let directory = tempfile::tempdir()?;
    let empty = directory.path().join("empty");
    let full = directory.path().join("full");
    let large = directory.path().join("large");
    let fifo = directory.path().join("fifo");
    std::fs::write(&empty, [])?;
    std::fs::write(&full, vec![0; MAX_OUTPUT_BYTES])?;
    std::fs::write(&large, vec![0; MAX_OUTPUT_BYTES + 1])?;
    mkfifo(&fifo)?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let cancel = CancellationToken::new();
        assert_eq!(runner.file(&empty, &cancel).await?, Some(Vec::new()));
        assert_eq!(runner.file(&directory.path().join("missing"), &cancel).await?, None);
        assert!(runner.exists(&empty, &cancel).await?);
        assert!(!runner.exists(&directory.path().join("missing"), &cancel).await?);
        assert!(runner.metadata(&empty.join("child"), &cancel).await.is_err());
        assert!(runner.file(&empty.join("child"), &cancel).await.is_err());
        assert_eq!(runner.file(&full, &cancel).await?.map(|bytes| bytes.len()), Some(MAX_OUTPUT_BYTES));
        assert!(matches!(runner.file(&large, &cancel).await, Err(AcquireError::OutputTooLarge)));
        assert!(matches!(runner.file(&fifo, &cancel).await, Err(AcquireError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput));
        Ok::<_, AcquireError>(())
    })?;
    Ok(())
}

/// Covers: T07
#[test]
fn cancelled_blocking_jobs_keep_both_slots_until_their_actual_completion() -> TestResult {
    runtime()?.block_on(async {
        let runner = Runner::default();
        let cancel = CancellationToken::new();
        let mut releases = Vec::new();
        let mut jobs = Vec::new();
        for _ in 0..2 {
            let (release, blocked) = mpsc::channel();
            let (started, ready) = oneshot::channel();
            let job_runner = runner.clone();
            let job_cancel = cancel.clone();
            jobs.push(tokio::spawn(async move {
                job_runner
                    .blocking(&job_cancel, move || {
                        let _ = started.send(());
                        blocked.recv().map_err(io::Error::other)?;
                        Ok(())
                    })
                    .await
            }));
            releases.push(release);
            ready.await?;
        }
        cancel.cancel();
        for job in jobs {
            assert!(matches!(job.await?, Err(AcquireError::Cancelled)));
        }
        assert_eq!(runner.files.available_permits(), 0);
        let ran = Arc::new(AtomicBool::new(false));
        let operation_ran = ran.clone();
        let replacement = runner
            .blocking(&CancellationToken::new(), move || {
                operation_ran.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await;
        assert!(matches!(replacement, Err(AcquireError::TimedOut)));
        assert!(!ran.load(Ordering::SeqCst));
        for release in releases {
            release.send(())?;
        }
        timeout(Duration::from_secs(5), async {
            while runner.files.available_permits() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(
            runner
                .blocking(&CancellationToken::new(), || Ok(42))
                .await?,
            42
        );
        Ok::<_, Box<dyn Error>>(())
    })?;
    Ok(())
}

#[test]
fn deduplicating_runner_shares_identical_commands_within_one_scope() -> TestResult {
    let directory = tempfile::tempdir()?;
    let count = directory.path().join("count");
    let counted = |label: &str| {
        argv(&[
            "sh",
            "-c",
            &format!("printf x >> {}; printf {label}", count.display()),
        ])
    };
    let (same, other) = (counted("same")?, counted("other")?);
    let input = snapshot(directory.path())?;
    let changed = with_env(
        directory.path(),
        input
            .vars()
            .iter()
            .cloned()
            .chain([("CAPSULE_SCOPE".into(), "changed".into())]),
    )?;
    let runs = || std::fs::read(&count).map(|bytes| bytes.len());
    runtime()?.block_on(async {
        let cancel = CancellationToken::new();
        let scope = Runner::default().deduplicating();
        let (first, second) = tokio::join!(
            scope.command(&same, &input, &cancel),
            scope.command(&same, &input, &cancel)
        );
        assert_eq!((first?, second?), (b"same".to_vec(), b"same".to_vec()));
        assert_eq!(scope.command(&same, &input, &cancel).await?, b"same");
        assert_eq!(runs()?, 1, "identical argv, cwd, and environment run once");

        assert_eq!(scope.command(&other, &input, &cancel).await?, b"other");
        assert_eq!(scope.command(&same, &changed, &cancel).await?, b"same");
        assert_eq!(
            runs()?,
            3,
            "a different argv or environment runs separately"
        );

        scope
            .deduplicating()
            .command(&same, &input, &cancel)
            .await?;
        Runner::default().command(&same, &input, &cancel).await?;
        Runner::default().command(&same, &input, &cancel).await?;
        assert_eq!(runs()?, 6, "fresh scopes and plain runners run every call");
        Ok::<_, Box<dyn Error>>(())
    })?;
    Ok(())
}

#[test]
fn deduplicating_runner_shares_failures() -> TestResult {
    let directory = tempfile::tempdir()?;
    let count = directory.path().join("count");
    let failing = argv(&[
        "sh",
        "-c",
        &format!("printf x >> {}; exit 7", count.display()),
    ])?;
    let input = snapshot(directory.path())?;
    runtime()?.block_on(async {
        let cancel = CancellationToken::new();
        let scope = Runner::default().deduplicating();
        for _ in 0..2 {
            let failure = scope.command(&failing, &input, &cancel).await;
            assert!(
                matches!(failure, Err(AcquireError::FailedExit(status)) if status.code() == Some(7))
            );
        }
        assert_eq!(std::fs::read(&count)?, b"x", "a shared failure runs once");
        Ok::<_, Box<dyn Error>>(())
    })?;
    Ok(())
}
