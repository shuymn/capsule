//! Worker boundaries use isolated homes and complete generation responses.

use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};

use capsule_protocol::session::{FrameReader, MAX_RESPONSE, Request, Snapshot};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, ChildStdout, Command},
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Worker {
    child: Child,
    input: ChildStdin,
    output: FrameReader<ChildStdout>,
}

impl Worker {
    fn start(home: &Path) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_capsule"))
            .arg("worker")
            .env_clear()
            .env("HOME", home)
            .env("PATH", "/usr/bin:/bin")
            .env("DISPLAY_VAR", "worker-only-value")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().ok_or("missing child stdin")?;
        let output = FrameReader::new(
            child.stdout.take().ok_or("missing child stdout")?,
            MAX_RESPONSE,
        );
        Ok(Self {
            child,
            input,
            output,
        })
    }

    async fn render(&mut self, request: &Request) -> Result<String> {
        self.input.write_all(&request.encode()?).await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = self.output.next().await? {
                let fields = frame.split(|byte| *byte == b'\t').collect::<Vec<_>>();
                if fields.len() == 5
                    && fields[0] == b"R"
                    && fields[1] == request.generation.to_string().as_bytes()
                    && fields[4] == b"1"
                {
                    return Ok(String::from_utf8(frame.to_vec())?);
                }
            }
            Err("worker closed before completing the generation".into())
        })
        .await?
    }

    async fn close(mut self) -> Result {
        self.input.shutdown().await?;
        drop(self.input);
        assert!(
            tokio::time::timeout(Duration::from_secs(3), self.child.wait())
                .await??
                .success()
        );
        Ok(())
    }
}

fn request(home: &Path, generation: u64) -> Request {
    Request {
        generation,
        cols: 100,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".to_owned(),
        snapshot: Snapshot {
            cwd: home.to_owned(),
            env: vec![
                (OsString::from("HOME"), home.as_os_str().to_owned()),
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (
                    OsString::from("COUNT_FILE"),
                    home.join("count").into_os_string(),
                ),
            ],
        },
    }
}

fn write_config(home: &Path, text: &str) -> Result {
    std::fs::create_dir_all(home.join(".capsule"))?;
    std::fs::write(home.join(".capsule/config.toml"), text)?;
    Ok(())
}

const CONFIG: &str = r#"schema_version = 2
[git]
disabled = true
[[module]]
name = "environment"
format = "VALUE={value}"
[module.values]
value = [{ command = ["/bin/sh", "-c", "printf x >> \"$COUNT_FILE\"; printf '%s' \"${DISPLAY_VAR-unset}\""] }]
"#;

#[tokio::test]
async fn same_generation_rerenders_without_reacquiring_and_environment_is_replaced() -> Result {
    let home = tempfile::tempdir()?;
    write_config(home.path(), CONFIG)?;
    let mut worker = Worker::start(home.path())?;
    let mut input = request(home.path(), 1);
    input
        .snapshot
        .env
        .push(("DISPLAY_VAR".into(), "first".into()));
    let initial = worker.render(&input).await?;
    assert!(
        initial.contains("VALUE=first"),
        "render={initial:?}, count={:?}",
        std::fs::read(home.path().join("count"))
    );
    assert_eq!(std::fs::read(home.path().join("count"))?, b"x");
    input.cols = 60;
    input.keymap = "vicmd".to_owned();
    let rerender = worker.render(&input).await?;
    assert!(rerender.contains("VALUE=first"));
    assert!(rerender.contains('❮'));
    assert_eq!(std::fs::read(home.path().join("count"))?, b"x");
    input.generation = 2;
    input.snapshot.env.retain(|(name, _)| name != "DISPLAY_VAR");
    assert!(worker.render(&input).await?.contains("VALUE=unset"));
    assert_eq!(std::fs::read(home.path().join("count"))?, b"xx");
    worker.close().await
}

#[tokio::test]
async fn failed_reload_preserves_last_valid_plan_then_accepts_fixed_configuration() -> Result {
    let home = tempfile::tempdir()?;
    write_config(home.path(), CONFIG)?;
    let mut worker = Worker::start(home.path())?;
    let initial = worker.render(&request(home.path(), 1)).await?;
    assert!(
        initial.contains("VALUE=unset"),
        "render={initial:?}, count={:?}",
        std::fs::read(home.path().join("count"))
    );
    write_config(home.path(), "schema_version = 2\nunknown = true\n")?;
    assert!(
        worker
            .render(&request(home.path(), 2))
            .await?
            .contains("VALUE=unset")
    );
    write_config(home.path(), &CONFIG.replace("VALUE=", "UPDATED="))?;
    assert!(
        worker
            .render(&request(home.path(), 3))
            .await?
            .contains("UPDATED=unset")
    );
    assert_eq!(std::fs::read(home.path().join("count"))?, b"xxx");
    worker.close().await
}

#[tokio::test]
async fn oversized_snapshot_is_rejected_without_running_a_command() -> Result {
    let home = tempfile::tempdir()?;
    write_config(home.path(), CONFIG)?;
    let mut worker = Worker::start(home.path())?;
    let mut frame = b"Q\t1\t0\t\t80\tmain\t/tmp\tOVERSIZE\t".to_vec();
    frame.resize(capsule_protocol::session::MAX_REQUEST + 1, b'x');
    frame.push(b'\n');
    let _write_result = worker.input.write_all(&frame).await;
    let status = tokio::time::timeout(Duration::from_secs(3), worker.child.wait()).await??;
    assert!(!status.success());
    assert!(!home.path().join("count").exists());
    Ok(())
}

#[tokio::test]
async fn real_git_directory_and_optional_time_survive_acquisition_completion() -> Result {
    let home = tempfile::tempdir()?;
    let repo = home.path().join("project");
    std::fs::create_dir_all(repo.join("src"))?;
    let init = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    std::fs::write(repo.join("src/untracked"), "fixture")?;
    write_config(
        home.path(),
        "schema_version = 2\n[time]\ndisabled = false\n",
    )?;
    let mut worker = Worker::start(home.path())?;
    let mut input = request(home.path(), 1);
    input.snapshot.cwd = repo.join("src");
    let rendered = worker.render(&input).await?;
    let response = capsule_protocol::session::Response::decode(rendered.as_bytes())?;
    assert!(response.left1.contains("src"), "{response:?}");
    assert!(!response.left1.contains("project/"), "{response:?}");
    assert!(response.left1.contains("main"), "{response:?}");
    assert!(response.left1.contains('?'), "{response:?}");
    assert!(
        response.left2.contains(':'),
        "optional time missing: {response:?}"
    );
    worker.close().await
}

#[tokio::test]
async fn relative_config_paths_follow_the_request_cwd() -> Result {
    let home = tempfile::tempdir()?;
    let mut worker = Worker::start(home.path())?;
    for (generation, directory, xdg) in [(1, "first", ""), (2, "second", "settings")] {
        let cwd = home.path().join(directory);
        let config_dir = cwd.join(xdg).join("capsule");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::write(
            config_dir.join("config.toml"),
            format!("schema_version = 2\n[character]\nglyph = '{directory}'\n"),
        )?;
        let mut input = request(home.path(), generation);
        input.snapshot.cwd = cwd;
        input
            .snapshot
            .env
            .push(("XDG_CONFIG_HOME".into(), xdg.into()));
        let rendered = worker.render(&input).await?;
        let response = capsule_protocol::session::Response::decode(rendered.as_bytes())?;
        assert!(response.left2.contains(directory), "{response:?}");
    }
    worker.close().await
}
