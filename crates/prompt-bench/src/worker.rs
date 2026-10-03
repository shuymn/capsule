//! Bounded adapter for the same session protocol used by the zsh integration.

use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::Context;
use capsule_prompt_bench::ACQUISITION_WAIT_SECS;
use capsule_protocol::session::{FrameReader, MAX_RESPONSE, Request, Response};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt as _},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use super::{BenchmarkEnvironment, ToolchainProbe};

pub struct Worker {
    child: Child,
    connection: Option<Connection<ChildStdout, ChildStdin>>,
}

pub struct Sample {
    pub(super) initial_ms: f64,
    pub(super) completed_ms: f64,
}

impl Worker {
    pub(super) fn spawn(
        binary: &Path,
        environment: &BenchmarkEnvironment<'_>,
    ) -> anyhow::Result<Self> {
        let mut child = Command::new(binary)
            .arg("worker")
            .env_clear()
            .env("HOME", environment.home_dir)
            .env("XDG_CONFIG_HOME", environment.home_dir.join(".config"))
            .env("PATH", environment.path_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("start capsule worker")?;
        let output = child.stdout.take().context("worker stdout unavailable")?;
        let input = child.stdin.take().context("worker stdin unavailable")?;
        Ok(Self {
            child,
            connection: Some(Connection::new(output, input)),
        })
    }

    pub(super) async fn measure(
        &mut self,
        request: &Request,
        probe: &ToolchainProbe,
        expected_calls: usize,
    ) -> anyhow::Result<Sample> {
        self.connection
            .as_mut()
            .context("worker already stopped")?
            .measure(
                request,
                probe,
                expected_calls,
                Duration::from_secs(ACQUISITION_WAIT_SECS),
            )
            .await
    }

    pub(super) async fn shutdown(&mut self) -> anyhow::Result<()> {
        // Closing stdin asks the worker to cancel acquisitions and reap children.
        let Some(Connection { mut frames, input }) = self.connection.take() else {
            return Ok(());
        };
        drop(input);
        // Keep draining stdout while the worker finishes; closing its read end
        // first would turn a queued final response into an artificial pipe error.
        let exited = async {
            loop {
                tokio::select! {
                    result = self.child.wait() => return result,
                    frame = frames.next() => match frame {
                        Ok(Some(_)) => {},
                        Ok(None) => return self.child.wait().await,
                        Err(error) => return Err(std::io::Error::other(error)),
                    }
                }
            }
        };
        let status = if let Ok(status) = tokio::time::timeout(Duration::from_secs(3), exited).await
        {
            status?
        } else {
            self.child.kill().await?;
            anyhow::bail!("worker did not stop within the cleanup deadline");
        };
        anyhow::ensure!(status.success(), "worker exited with {status}");
        Ok(())
    }
}

struct Connection<R, W> {
    frames: FrameReader<R>,
    input: W,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Connection<R, W> {
    fn new(output: R, input: W) -> Self {
        Self {
            frames: FrameReader::new(output, MAX_RESPONSE),
            input,
        }
    }

    async fn measure(
        &mut self,
        request: &Request,
        probe: &ToolchainProbe,
        expected_calls: usize,
        timeout: Duration,
    ) -> anyhow::Result<Sample> {
        let before = probe.counts()?;
        let expected_character = if request.keymap == "vicmd" {
            '❮'
        } else {
            '❯'
        };
        let frame = request.encode()?;
        let started = Instant::now();
        let sample = tokio::time::timeout(timeout, async {
            self.input
                .write_all(&frame)
                .await
                .context("send worker request")?;
            self.input.flush().await?;
            let mut initial_ms = None;
            loop {
                let frame = self
                    .frames
                    .next()
                    .await?
                    .context("worker closed before completion")?;
                if frame.as_ref() == b"K" {
                    continue;
                }
                let response = Response::decode(&frame)?;
                if response.generation != request.generation
                    || !response.left2.contains(expected_character)
                {
                    continue;
                }
                let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                let initial_ms = *initial_ms.get_or_insert(elapsed);
                if response.complete {
                    if expected_calls > 0 || request.snapshot.cwd.join("Cargo.toml").is_file() {
                        anyhow::ensure!(
                            response.left1.contains(&probe.version),
                            "completed worker prompt is missing rustc output"
                        );
                    }
                    return Ok(Sample {
                        initial_ms,
                        completed_ms: elapsed,
                    });
                }
            }
        })
        .await
        .context("worker completion deadline exceeded")??;
        probe.verify(before, expected_calls)?;
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use capsule_protocol::session::{self, MAX_REQUEST, Snapshot};
    use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

    use super::*;

    fn probe(root: &Path) -> anyhow::Result<ToolchainProbe> {
        let log_path = root.join("calls");
        fs::write(&log_path, "")?;
        fs::write(root.join("Cargo.toml"), "[package]\nname='fixture'\n")?;
        Ok(ToolchainProbe {
            bin_dir: root.into(),
            log_path,
            version: "rustc fixture".into(),
        })
    }

    fn request(cwd: &Path, keymap: &str) -> Request {
        Request {
            generation: 7,
            snapshot: Snapshot {
                cwd: cwd.into(),
                env: Vec::new(),
            },
            cols: 240,
            last_exit_code: 0,
            duration_ms: None,
            keymap: keymap.into(),
        }
    }

    fn connection() -> (
        Connection<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>,
        DuplexStream,
    ) {
        let (client, server) = tokio::io::duplex(32);
        let (read, write) = tokio::io::split(client);
        (Connection::new(read, write), server)
    }

    async fn read_request(
        server: DuplexStream,
    ) -> anyhow::Result<(Request, WriteHalf<DuplexStream>)> {
        let (read, write) = tokio::io::split(server);
        let mut frames = FrameReader::new(read, MAX_REQUEST);
        let frame = frames.next().await?.context("missing request")?;
        Ok((Request::decode(&frame)?, write))
    }

    #[tokio::test]
    async fn coalesced_final_response_proves_work_and_handles_credit_and_partial_frames()
    -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let probe = probe(temp.path())?;
        let log = probe.log_path.clone();
        let (mut connection, server) = connection();
        let task = tokio::spawn(async move {
            let (request, mut output) = read_request(server).await?;
            fs::write(log, "start\nfinish:0\n")?;
            output.write_all(b"K\n").await?;
            for generation in [request.generation - 1, request.generation + 1] {
                output
                    .write_all(&session::response(
                        generation,
                        "wrong generation",
                        "❯",
                        true,
                    )?)
                    .await?;
            }
            for chunk in
                session::response(request.generation, "rustc fixture", "❯", true)?.chunks(3)
            {
                output.write_all(chunk).await?;
            }
            anyhow::Ok(())
        });
        let sample = connection
            .measure(
                &request(temp.path(), "main"),
                &probe,
                1,
                Duration::from_secs(2),
            )
            .await?;
        assert!((sample.initial_ms - sample.completed_ms).abs() < f64::EPSILON);
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn initial_response_without_completion_fails_on_eof() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let probe = probe(temp.path())?;
        let (mut connection, server) = connection();
        let task = tokio::spawn(async move {
            let (request, mut output) = read_request(server).await?;
            output
                .write_all(&session::response(
                    request.generation,
                    "rustc fixture",
                    "❯",
                    false,
                )?)
                .await?;
            anyhow::Ok(())
        });
        let result = connection
            .measure(
                &request(temp.path(), "main"),
                &probe,
                1,
                Duration::from_secs(2),
            )
            .await;
        assert!(result.is_err_and(|error| error.to_string().contains("closed before completion")));
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn initial_response_without_completion_obeys_the_deadline() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let probe = probe(temp.path())?;
        let (mut connection, server) = connection();
        let task = tokio::spawn(async move {
            let (request, mut output) = read_request(server).await?;
            output
                .write_all(&session::response(
                    request.generation,
                    "rustc fixture",
                    "❯",
                    false,
                )?)
                .await?;
            std::future::pending::<()>().await;
            anyhow::Ok(())
        });
        let result = connection
            .measure(
                &request(temp.path(), "main"),
                &probe,
                1,
                Duration::from_millis(30),
            )
            .await;
        assert!(result.is_err_and(|error| error.to_string().contains("completion deadline")));
        task.abort();
        assert!(task.await.is_err_and(|error| error.is_cancelled()));
        Ok(())
    }

    #[tokio::test]
    async fn same_generation_redraw_skips_old_glyph_and_requires_no_acquisition()
    -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let probe = probe(temp.path())?;
        fs::write(&probe.log_path, "start\nfinish:0\n")?;
        let (mut connection, server) = connection();
        let task = tokio::spawn(async move {
            let (request, mut output) = read_request(server).await?;
            assert_eq!(request.keymap, "vicmd");
            output
                .write_all(&session::response(
                    request.generation,
                    "old duplicate",
                    "❯",
                    true,
                )?)
                .await?;
            output
                .write_all(&session::response(
                    request.generation,
                    "rustc fixture",
                    "❮",
                    true,
                )?)
                .await?;
            anyhow::Ok(())
        });
        connection
            .measure(
                &request(temp.path(), "vicmd"),
                &probe,
                0,
                Duration::from_secs(2),
            )
            .await?;
        assert_eq!(probe.counts()?.completed, 1);
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn completion_cannot_mask_failed_or_unexpected_acquisition() -> anyhow::Result<()> {
        for (record, expected) in [("start\nfinish:1\n", 1), ("start\nfinish:0\n", 0), ("", 1)] {
            let temp = tempfile::tempdir()?;
            let probe = probe(temp.path())?;
            let log = probe.log_path.clone();
            let (mut connection, server) = connection();
            let task = tokio::spawn(async move {
                let (request, mut output) = read_request(server).await?;
                fs::write(log, record)?;
                output
                    .write_all(&session::response(
                        request.generation,
                        "rustc fixture",
                        "❯",
                        true,
                    )?)
                    .await?;
                anyhow::Ok(())
            });
            let result = connection
                .measure(
                    &request(temp.path(), "main"),
                    &probe,
                    expected,
                    Duration::from_secs(2),
                )
                .await;
            assert!(result.is_err_and(|error| error.to_string().contains("acquisition mismatch")));
            task.await??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_closes_input_and_drains_output_before_reaping() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let probe = probe(temp.path())?;
        let binary = temp.path().join("worker");
        let shell = capsule_prompt_bench::resolve_binary(Path::new("sh"), "shell")?;
        fs::write(
            &binary,
            format!(
                "#!{}\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
                shell.display(),
                "K".repeat(MAX_RESPONSE - 1)
            ),
        )?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
        let path_env = std::env::var("PATH")?;
        let environment = BenchmarkEnvironment {
            home_dir: temp.path(),
            probe: &probe,
            path_env: &path_env,
        };
        let mut worker = Worker::spawn(&binary, &environment)?;
        worker.shutdown().await?;
        assert!(worker.child.try_wait()?.is_some());
        Ok(())
    }
}
