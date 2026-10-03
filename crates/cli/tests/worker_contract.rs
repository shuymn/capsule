//! Worker boundaries use isolated homes and inspect pending and complete responses.

use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::Write,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use capsule_protocol::session::{FrameReader, MAX_RESPONSE, Request, Response, Snapshot};
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
            .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
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

    async fn send(&mut self, request: &Request) -> Result {
        self.input.write_all(&request.encode()?).await?;
        Ok(())
    }

    async fn next_response(&mut self) -> Result<Response> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = self.output.next().await? {
                if frame == b"K"[..] {
                    continue;
                }
                return Ok(Response::decode(&frame)?);
            }
            Err("worker closed before responding".into())
        })
        .await?
    }

    async fn generation_responses(&mut self, generation: u64) -> Result<Vec<Response>> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut responses = Vec::new();
            loop {
                let response = self.next_response().await?;
                if response.generation < generation {
                    continue;
                }
                assert_eq!(response.generation, generation, "{response:?}");
                let complete = response.complete;
                responses.push(response);
                if complete {
                    return Ok(responses);
                }
            }
        })
        .await?
    }

    async fn render(&mut self, request: &Request) -> Result<String> {
        self.send(request).await?;
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
                (
                    OsString::from("PATH"),
                    std::env::var_os("PATH").unwrap_or_default(),
                ),
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

// Each command announces entry before blocking on the FIFO. Keep a writer open so
// cancellation cannot turn a replacement's read into EOF. Releases, not sleeps,
// determine completion within the worker's 500 ms operation / 2 s generation bounds.
struct Gate {
    fifo: File,
    entered: PathBuf,
}

impl Gate {
    fn new(cwd: &Path) -> Result<Self> {
        let path = cwd.join("gate");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()?
                .success()
        );
        Ok(Self {
            fifo: OpenOptions::new().read(true).write(true).open(path)?,
            entered: cwd.join("entered"),
        })
    }

    async fn wait_for_entry(&self, count: usize) -> Result {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match std::fs::read(&self.entered) {
                    Ok(bytes) if bytes.len() == count => return Ok(()),
                    Ok(bytes) if bytes.len() > count => {
                        return Err("unexpected extra acquisition".into());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await?
    }

    fn release(&mut self) -> Result {
        self.fifo.write_all(b"release\n")?;
        Ok(())
    }
}

fn settled(responses: &[Response]) -> Result<&Response> {
    let response = responses.last().ok_or("missing generation response")?;
    assert!(response.complete, "{response:?}");
    Ok(response)
}

fn assert_retained(response: &Response, values: &[&str]) {
    assert!(
        !response.complete,
        "gate completed before release: {response:?}"
    );
    for value in values {
        assert!(response.left1.contains(value), "lost {value}: {response:?}");
    }
    assert!(
        !response.left1.contains("=new"),
        "early commit: {response:?}"
    );
}

fn assert_external_hidden(response: &Response) {
    assert!(
        !response.complete,
        "gate completed before release: {response:?}"
    );
    for prefix in ["FAST=", "SLOW=", "RECONFIGURED="] {
        assert!(
            !response.left1.contains(prefix),
            "unvalidated external information: {response:?}"
        );
    }
}

const RETAINED_CONFIG: &str = r#"schema_version = 2
[git]
disabled = true
[character]
glyph = "❯"
success_style = { fg = "green", bold = false }
error_style = { fg = "red", bold = false }
[[module]]
name = "fast"
format = "FAST={value}"
[module.values]
value = [{ file = "fast" }]
[[module]]
name = "slow"
format = "SLOW={value}"
[module.values]
value = [{ command = ["sh", "-c", "printf x >> entered; IFS= read -r release < gate; cat slow"] }]
[[module]]
name = "missing"
format = "MISSING={value}"
[module.values]
value = [{ file = "missing" }]
[[module]]
name = "failed"
format = "FAILED={value}"
[module.values]
value = [{ file = "failed" }]
[[module]]
name = "conditional"
when.files = ["condition"]
format = "CONDITION={value}"
[module.values]
value = [{ file = "condition" }]
"#;

const CONFIG: &str = r#"schema_version = 2
[git]
disabled = true
[[module]]
name = "environment"
format = "VALUE={value}"
[module.values]
value = [{ command = ["sh", "-c", "printf x >> \"$COUNT_FILE\"; printf '%s' \"${DISPLAY_VAR-unset}\""] }]
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
async fn same_snapshot_retains_all_pending_frames_and_commits_external_changes_together() -> Result
{
    let home = tempfile::tempdir()?;
    let directory = "directory-with-a-very-long-name-to-exercise-live-width-changes";
    let cwd = home.path().join(directory);
    std::fs::create_dir(&cwd)?;
    for name in ["fast", "slow", "missing", "failed", "condition"] {
        std::fs::write(cwd.join(name), "old")?;
    }
    write_config(home.path(), RETAINED_CONFIG)?;
    let mut gate = Gate::new(&cwd)?;
    let mut worker = Worker::start(home.path())?;
    let mut input = request(home.path(), 1);
    input.snapshot.cwd = cwd.clone();
    input.cols = 160;
    worker.send(&input).await?;
    gate.wait_for_entry(1).await?;
    gate.release()?;
    let initial = worker.generation_responses(1).await?;
    let retained = [
        "FAST=old",
        "SLOW=old",
        "MISSING=old",
        "FAILED=old",
        "CONDITION=old",
    ];
    for value in retained {
        assert!(settled(&initial)?.left1.contains(value), "{initial:?}");
    }

    std::fs::write(cwd.join("fast"), "new")?;
    std::fs::write(cwd.join("slow"), "new")?;
    std::fs::remove_file(cwd.join("missing"))?;
    std::fs::remove_file(cwd.join("failed"))?;
    std::fs::create_dir(cwd.join("failed"))?;
    std::fs::remove_file(cwd.join("condition"))?;
    input.generation = 2;
    input.snapshot.env.reverse(); // Enumeration order is not an environment change.
    worker.send(&input).await?;
    gate.wait_for_entry(2).await?;
    let first = loop {
        let response = worker.next_response().await?;
        if response.generation == 2 {
            assert_retained(&response, &retained);
            break response;
        }
    };
    assert!(first.left1.contains(directory), "{first:?}");

    // Local input changes must be visible even though no acquisition can finish.
    input.cols = 80;
    input.keymap = "vicmd".to_owned();
    input.last_exit_code = 7;
    input.duration_ms = Some(65_000);
    worker.send(&input).await?;
    loop {
        let response = worker.next_response().await?;
        assert_eq!(response.generation, 2, "{response:?}");
        assert_retained(&response, &retained);
        if response.left2.contains('❮') {
            assert!(response.left2.contains("\x1b[31m"), "{response:?}");
            assert!(response.left1.contains("1m5s"), "{response:?}");
            assert!(!response.left1.contains(directory), "{response:?}");
            break;
        }
    }
    gate.release()?;
    let remaining = worker.generation_responses(2).await?;
    for response in remaining.iter().filter(|response| !response.complete) {
        assert_retained(response, &retained);
    }
    let complete = settled(&remaining)?;
    assert!(complete.left1.contains("FAST=new"), "{complete:?}");
    assert!(complete.left1.contains("SLOW=new"), "{complete:?}");
    for removed in ["=old", "MISSING=", "FAILED=", "CONDITION="] {
        assert!(!complete.left1.contains(removed), "{complete:?}");
    }
    assert!(complete.left1.contains("1m5s"), "{complete:?}");
    assert!(complete.left2.contains('❮'), "{complete:?}");
    worker.close().await
}

#[tokio::test]
async fn cancellation_and_failed_reload_retain_the_last_settled_generation() -> Result {
    let home = tempfile::tempdir()?;
    write_config(home.path(), RETAINED_CONFIG)?;
    std::fs::write(home.path().join("fast"), "old")?;
    std::fs::write(home.path().join("slow"), "old")?;
    let mut gate = Gate::new(home.path())?;
    let mut worker = Worker::start(home.path())?;
    let mut input = request(home.path(), 1);
    worker.send(&input).await?;
    gate.wait_for_entry(1).await?;
    gate.release()?;
    let initial = worker.generation_responses(1).await?;
    assert!(settled(&initial)?.left1.contains("SLOW=old"), "{initial:?}");

    std::fs::write(home.path().join("fast"), "cancelled")?;
    std::fs::write(home.path().join("slow"), "cancelled")?;
    input.generation = 2;
    worker.send(&input).await?;
    gate.wait_for_entry(2).await?;
    loop {
        let response = worker.next_response().await?;
        if response.generation == 2 {
            assert_retained(&response, &["FAST=old", "SLOW=old"]);
            break;
        }
    }
    let mut stale = input.clone();
    stale.keymap = "vicmd".to_owned();
    write_config(home.path(), "schema_version = 2\nunknown = true\n")?;
    std::fs::write(home.path().join("fast"), "replacement")?;
    std::fs::write(home.path().join("slow"), "replacement")?;
    input.generation = 3;
    worker.send(&input).await?;
    gate.wait_for_entry(3).await?;
    loop {
        let response = worker.next_response().await?;
        if response.generation >= 2 {
            assert_retained(&response, &["FAST=old", "SLOW=old"]);
            assert!(!response.left1.contains("=cancelled"), "{response:?}");
            assert!(!response.left1.contains("=replacement"), "{response:?}");
        }
        if response.generation == 3 {
            break;
        }
    }

    // A stale request cannot select command mode or revive the cancelled work.
    worker.send(&stale).await?;
    input.duration_ms = Some(3_000);
    worker.send(&input).await?;
    loop {
        let response = worker.next_response().await?;
        assert_eq!(response.generation, 3, "{response:?}");
        assert_retained(&response, &["FAST=old", "SLOW=old"]);
        assert!(!response.left1.contains("=replacement"), "{response:?}");
        assert!(
            !response.left2.contains('❮'),
            "stale request applied: {response:?}"
        );
        if response.left1.contains("3s") {
            break;
        }
    }
    gate.release()?;
    let remaining = worker.generation_responses(3).await?;
    for response in remaining.iter().filter(|response| !response.complete) {
        assert_retained(response, &["FAST=old", "SLOW=old"]);
        assert!(!response.left1.contains("=replacement"), "{response:?}");
    }
    let complete = settled(&remaining)?;
    assert!(complete.left1.contains("FAST=replacement"), "{complete:?}");
    assert!(complete.left1.contains("SLOW=replacement"), "{complete:?}");
    assert!(!complete.left1.contains("=cancelled"), "{complete:?}");
    assert_eq!(std::fs::read(home.path().join("entered"))?, b"xxx");
    worker.close().await
}

#[tokio::test]
async fn changed_environment_cwd_and_valid_config_clear_retained_information() -> Result {
    let home = tempfile::tempdir()?;
    write_config(home.path(), RETAINED_CONFIG)?;
    std::fs::write(home.path().join("fast"), "old")?;
    std::fs::write(home.path().join("slow"), "old")?;
    let mut gate = Gate::new(home.path())?;
    let mut worker = Worker::start(home.path())?;
    let mut input = request(home.path(), 1);
    input
        .snapshot
        .env
        .push(("UNUSED".into(), OsString::from_vec(vec![b'v', 0xff])));
    worker.send(&input).await?;
    gate.wait_for_entry(1).await?;
    gate.release()?;
    let initial = worker.generation_responses(1).await?;
    assert!(settled(&initial)?.left1.contains("SLOW=old"), "{initial:?}");

    // Even an unused exported variable's non-UTF-8 bytes belong to the snapshot.
    input.generation = 2;
    let (_, unused) = input.snapshot.env.last_mut().ok_or("missing UNUSED")?;
    *unused = OsString::from_vec(vec![b'v', 0xfe]);
    std::fs::write(home.path().join("fast"), "env-new")?;
    std::fs::write(home.path().join("slow"), "env-new")?;
    worker.send(&input).await?;
    gate.wait_for_entry(2).await?;
    loop {
        let response = worker.next_response().await?;
        if response.generation == 2 {
            assert_external_hidden(&response);
            break;
        }
    }
    gate.release()?;
    let environment = worker.generation_responses(2).await?;
    for response in environment.iter().filter(|response| !response.complete) {
        assert_external_hidden(response);
    }
    assert!(
        settled(&environment)?.left1.contains("SLOW=env-new"),
        "{environment:?}"
    );

    let cwd = home.path().join("different-cwd");
    std::fs::create_dir(&cwd)?;
    std::fs::write(cwd.join("fast"), "cwd-new")?;
    std::fs::write(cwd.join("slow"), "cwd-new")?;
    let mut cwd_gate = Gate::new(&cwd)?;
    input.generation = 3;
    input.snapshot.cwd = cwd.clone();
    worker.send(&input).await?;
    cwd_gate.wait_for_entry(1).await?;
    loop {
        let response = worker.next_response().await?;
        if response.generation == 3 {
            assert_external_hidden(&response);
            assert!(response.left1.contains("different-cwd"), "{response:?}");
            break;
        }
    }
    cwd_gate.release()?;
    let directory = worker.generation_responses(3).await?;
    for response in directory.iter().filter(|response| !response.complete) {
        assert_external_hidden(response);
    }
    assert!(
        settled(&directory)?.left1.contains("SLOW=cwd-new"),
        "{directory:?}"
    );

    // The new character witnesses Plan application: retention is allowed before
    // config validation, but no old or partially acquired module survives it.
    write_config(
        home.path(),
        &RETAINED_CONFIG
            .replace("FAST=", "RECONFIGURED=")
            .replace("glyph = \"❯\"", "glyph = \"reconfigured\""),
    )?;
    std::fs::write(cwd.join("fast"), "config-new")?;
    std::fs::write(cwd.join("slow"), "config-new")?;
    input.generation = 4;
    worker.send(&input).await?;
    cwd_gate.wait_for_entry(2).await?;
    loop {
        let response = worker.next_response().await?;
        if response.generation == 4 && response.left2.contains("reconfigured") {
            assert_external_hidden(&response);
            break;
        }
    }
    cwd_gate.release()?;
    let configuration = worker.generation_responses(4).await?;
    for response in configuration.iter().filter(|response| !response.complete) {
        assert_external_hidden(response);
    }
    let complete = settled(&configuration)?;
    assert!(
        complete.left1.contains("RECONFIGURED=config-new"),
        "{complete:?}"
    );
    assert!(complete.left1.contains("SLOW=config-new"), "{complete:?}");
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
        .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
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
