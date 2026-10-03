use std::{
    error::Error,
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use tokio::{runtime::Runtime, sync::oneshot};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn runtime() -> io::Result<Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn snapshot(cwd: &Path) -> Snapshot {
    Snapshot {
        cwd: cwd.to_owned(),
        env: std::env::var_os("PATH")
            .into_iter()
            .map(|value| (OsString::from("PATH"), value))
            .collect(),
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

#[test]
fn command_preserves_raw_environment_and_clears_unspecified_variables() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut input = snapshot(directory.path());
    input.env = vec![(
        OsString::from("CAPSULE_VALUE"),
        OsString::from_vec(b"raw\xff\n\t\\".to_vec()),
    )];
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
    runtime()?.block_on(async {
        let runner = Runner::default();
        let input = snapshot(directory.path());
        let cancel = CancellationToken::new();
        let output = runner
            .command(&argv(&["pwd", "-P"])?, &input, &cancel)
            .await?;
        assert_eq!(output, format!("{}\n", expected.display()).as_bytes());
        let failure = runner
            .command(
                &argv(&["sh", "-c", "printf hidden >&2; exit 7"])?,
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

#[test]
fn command_rejects_excessive_stdout_and_releases_capacity() -> TestResult {
    let command = argv(&["sh", "-c", "printf '%65537s' x"])?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let output = runner
            .command(
                &command,
                &snapshot(Path::new("/")),
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(output, Err(AcquireError::OutputTooLarge)));
        assert_eq!(runner.processes.available_permits(), 4);
    });
    Ok(())
}

#[test]
fn timeout_stops_descendants_that_hold_stdout_after_the_leader_exits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let liveness = directory.path().join("liveness");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&liveness)
            .status()?
            .success()
    );
    let fd = rustix::fs::open(
        &liveness,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let mut reader = File::from(fd);
    let command = argv(&[
        "sh",
        "-c",
        "sh -c 'exec 3>liveness; printf ready >&3; sleep 60' & exit 0",
    ])?;
    runtime()?.block_on(async {
        let runner = Runner::default();
        let input = snapshot(directory.path());
        let result = timeout(
            Duration::from_secs(5),
            runner.command(&command, &input, &CancellationToken::new()),
        )
        .await;
        assert!(matches!(result, Ok(Err(AcquireError::TimedOut))));
        assert_eq!(runner.processes.available_permits(), 4);
    });
    // The descendant's extra pipe stays open while it lives. EOF after `ready`
    // proves termination; a leaked descendant would instead return WouldBlock.
    let mut observed = Vec::new();
    reader.read_to_end(&mut observed)?;
    assert_eq!(observed, b"ready");
    Ok(())
}

#[test]
fn cancellation_stops_running_commands_and_prevents_queued_spawns() -> TestResult {
    runtime()?.block_on(async {
        let runner = Runner::default();
        let cancel = CancellationToken::new();
        let command_runner = runner.clone();
        let command_cancel = cancel.clone();
        let command = tokio::spawn(async move {
            command_runner
                .command(
                    &argv(&["sleep", "60"])?,
                    &snapshot(Path::new("/")),
                    &command_cancel,
                )
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
            .command(
                &argv(&["/does/not/exist"])?,
                &snapshot(Path::new("/")),
                &cancel,
            )
            .await;
        assert!(matches!(queued, Err(AcquireError::Cancelled)));
        Ok::<_, Box<dyn Error>>(())
    })?;
    Ok(())
}

#[test]
fn dropped_command_future_retains_its_slot_until_the_child_is_reaped() -> TestResult {
    runtime()?.block_on(async {
        let runner = Runner::default();
        let command_runner = runner.clone();
        let command = tokio::spawn(async move {
            command_runner
                .command(
                    &argv(&["sleep", "60"])?,
                    &snapshot(Path::new("/")),
                    &CancellationToken::new(),
                )
                .await
        });
        timeout(Duration::from_secs(5), async {
            while runner.processes.available_permits() == 4 {
                tokio::task::yield_now().await;
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
    Ok(())
}

#[test]
fn file_distinguishes_missing_empty_oversized_and_non_regular_sources() -> TestResult {
    let directory = tempfile::tempdir()?;
    let empty = directory.path().join("empty");
    let large = directory.path().join("large");
    let fifo = directory.path().join("fifo");
    std::fs::write(&empty, [])?;
    std::fs::write(&large, vec![0; MAX_OUTPUT_BYTES + 1])?;
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()?
            .success()
    );
    runtime()?.block_on(async {
        let runner = Runner::default();
        let cancel = CancellationToken::new();
        assert_eq!(runner.file(&empty, &cancel).await?, Some(Vec::new()));
        assert_eq!(runner.file(&directory.path().join("missing"), &cancel).await?, None);
        assert!(runner.exists(&empty, &cancel).await?);
        assert!(!runner.exists(&directory.path().join("missing"), &cancel).await?);
        assert!(matches!(runner.file(&large, &cancel).await, Err(AcquireError::OutputTooLarge)));
        assert!(matches!(runner.file(&fifo, &cancel).await, Err(AcquireError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput));
        Ok::<_, AcquireError>(())
    })?;
    Ok(())
}

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
