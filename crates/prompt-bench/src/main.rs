//! Benchmark capsule (daemon socket) vs `starship prompt`.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::Context;
use capsule_prompt_bench::{
    DEFAULT_ITERATIONS, RENDER_RESULT_WAIT_SECS, RunMetadata, ScenarioResult, UPDATE_WAIT_MS,
    build_path_env, resolve_binary, summarize,
};
use capsule_protocol::{
    Message, PROTOCOL_VERSION, Request, SessionId, generation::PromptGeneration,
};
use clap::Parser;
use serde::Serialize;

/// Monotonic request counter shared across all benchmark connections. The daemon rejects
/// `generation` that does not increase per [`SessionId`], so this must not reset when
/// reconnecting per workload.
static CAPSULE_PROMPT_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Parser, Debug)]
#[command(about = "Benchmark capsule and starship prompt latency.")]
struct Args {
    /// Path to the capsule binary (expect a release build; default is `target/release/capsule`).
    #[arg(long, default_value = "target/release/capsule")]
    capsule_bin: PathBuf,

    /// Path to the starship binary.
    #[arg(long, default_value = "starship")]
    starship_bin: PathBuf,

    /// Path to the git binary.
    #[arg(long, default_value = "git")]
    git_bin: PathBuf,

    /// Samples per workload (excluding warm-up).
    #[arg(long, default_value_t = DEFAULT_ITERATIONS)]
    iterations: usize,

    /// Write JSON report to this path.
    #[arg(long)]
    json_out: Option<PathBuf>,

    /// Write Markdown report to this path.
    #[arg(long)]
    markdown_out: Option<PathBuf>,
}

struct Workload {
    path: PathBuf,
    description: String,
    subdirs: Vec<PathBuf>,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    metadata: &'a RunMetadata,
    results: &'a [ScenarioResult],
}

fn main() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        unix_main()
    }
    #[cfg(not(unix))]
    {
        eprintln!("prompt-bench: unix-only (requires unix domain sockets)");
        std::process::exit(2);
    }
}

#[cfg(unix)]
fn unix_main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.iterations < 1 {
        anyhow::bail!("--iterations must be at least 1");
    }

    let capsule_bin =
        resolve_binary(&args.capsule_bin, "capsule").context("resolve capsule binary")?;
    let starship_bin =
        resolve_binary(&args.starship_bin, "starship").context("resolve starship binary")?;
    let git_bin = resolve_binary(&args.git_bin, "git").context("resolve git binary")?;

    let temp = tempfile::Builder::new()
        .prefix("prompt-bench-")
        .tempdir()
        .context("create temp dir")?;
    let root = temp.path();
    let home_dir = root.join("home");
    fs::create_dir_all(home_dir.join(".capsule")).context("create fake HOME/.capsule")?;

    let workloads =
        create_workloads(root, &git_bin, args.iterations).context("create benchmark workloads")?;

    write_bench_config(&home_dir).context("write bench config")?;

    eprintln!("Starting capsule daemon...");
    let mut daemon = start_daemon(&capsule_bin, &home_dir).context("start daemon")?;

    let results = match run_benchmark(
        &workloads,
        &capsule_bin,
        &starship_bin,
        &git_bin,
        &home_dir,
        args.iterations,
    ) {
        Ok(r) => r,
        Err(e) => {
            stop_daemon(&mut daemon);
            return Err(e);
        }
    };

    stop_daemon(&mut daemon);

    let rustc = try_command_output(&["rustc", "-V"]);
    let metadata = RunMetadata {
        iterations: args.iterations,
        capsule_bin: capsule_bin.display().to_string(),
        starship_bin: starship_bin.display().to_string(),
        git_bin: git_bin.display().to_string(),
        rustc,
        macos: try_command_output(&["sw_vers", "-productVersion"]),
        kernel: try_command_output(&["uname", "-srv"]),
        cpu: try_command_output(&["sysctl", "-n", "machdep.cpu.brand_string"]),
    };

    let markdown = render_markdown(&metadata, &results);
    print!("{markdown}");

    if let Some(path) = args.markdown_out.as_ref() {
        fs::write(path, markdown.as_bytes()).with_context(|| path.display().to_string())?;
    }

    if let Some(path) = args.json_out.as_ref() {
        let report = JsonReport {
            metadata: &metadata,
            results: &results,
        };
        let json = serde_json::to_string_pretty(&report).context("serialize JSON report")?;
        fs::write(path, format!("{json}\n")).with_context(|| path.display().to_string())?;
    }

    Ok(())
}

fn format_ms(value: f64) -> String {
    format!("{value:.2}")
}

fn render_markdown(metadata: &RunMetadata, results: &[ScenarioResult]) -> String {
    let mut lines: Vec<String> = vec![
        "# Prompt Benchmark Report".to_owned(),
        String::new(),
        "capsule: direct daemon socket (fast = RenderResult, slow = +Update with git/toolchain)."
            .to_owned(),
        "starship: `starship prompt` subprocess.".to_owned(),
        String::new(),
        "## Environment".to_owned(),
        String::new(),
        format!("- Iterations per workload: `{}`", metadata.iterations),
        format!("- macOS: `{}`", metadata.macos),
        format!("- CPU: `{}`", metadata.cpu),
        format!("- capsule: `{}`", metadata.capsule_bin),
        format!("- starship: `{}`", metadata.starship_bin),
        String::new(),
        "## Results".to_owned(),
        String::new(),
        "| Workload | Tool | Fast p50 ms | Fast p95 ms | Slow p50 ms | Slow p95 ms | Description |"
            .to_owned(),
        "| --- | --- | ---: | ---: | ---: | ---: | --- |".to_owned(),
    ];

    for r in results {
        let (slow_p50, slow_p95) = r.slow.as_ref().map_or_else(
            || ("-".to_owned(), "-".to_owned()),
            |s| (format_ms(s.p50_ms), format_ms(s.p95_ms)),
        );
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.workload,
            r.tool,
            format_ms(r.fast.p50_ms),
            format_ms(r.fast.p95_ms),
            slow_p50,
            slow_p95,
            r.description
        ));
    }

    lines.join("\n") + "\n"
}

fn try_command_output(argv: &[&str]) -> String {
    if argv.is_empty() {
        return "unknown".to_owned();
    }
    let Ok(out) = Command::new(argv[0]).args(&argv[1..]).output() else {
        return "unknown".to_owned();
    };
    if !out.status.success() {
        return "unknown".to_owned();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn run_command(cmd: &[&str], cwd: &Path) -> anyhow::Result<()> {
    if cmd.is_empty() {
        anyhow::bail!("empty command");
    }
    let status = Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd)
        .status()
        .with_context(|| format!("run {}", cmd.join(" ")))?;
    if !status.success() {
        anyhow::bail!("command failed: {}", cmd.join(" "));
    }
    Ok(())
}

fn create_repo(repo: &Path, git_bin: &Path, file_count: usize) -> anyhow::Result<()> {
    fs::create_dir_all(repo).with_context(|| repo.display().to_string())?;
    let git = git_bin.to_str().context("git path is not valid UTF-8")?;
    run_command(&[git, "init", "-q"], repo)?;
    run_command(&[git, "config", "user.name", "Prompt Bench"], repo)?;
    run_command(
        &[git, "config", "user.email", "bench@example.invalid"],
        repo,
    )?;
    run_command(&[git, "config", "commit.gpgsign", "false"], repo)?;

    for index in 0..file_count {
        let nested = repo
            .join("src")
            .join(format!("group-{}", index % 8))
            .join(format!("file-{index:04}.txt"));
        let parent = nested
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing parent for {}", nested.display()))?;
        fs::create_dir_all(parent).with_context(|| nested.display().to_string())?;
        fs::write(&nested, format!("sample file {index}\n"))
            .with_context(|| nested.display().to_string())?;
    }

    run_command(&[git, "add", "."], repo)?;
    run_command(&[git, "commit", "-qm", "initial"], repo)?;
    Ok(())
}

fn create_toolchain_repo(repo: &Path, git_bin: &Path) -> anyhow::Result<()> {
    create_repo(repo, git_bin, 16)?;
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"prompt-bench-toolchain\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    fs::create_dir_all(repo.join("src"))?;
    fs::write(
        repo.join("src").join("main.rs"),
        "fn main() {\n    println!(\"toolchain marker\");\n}\n",
    )?;
    let git = git_bin.to_str().context("git path is not valid UTF-8")?;
    run_command(&[git, "add", "."], repo)?;
    run_command(&[git, "commit", "-qm", "toolchain"], repo)?;
    Ok(())
}

fn create_subdirs(base: &Path, count: usize) -> anyhow::Result<Vec<PathBuf>> {
    let mut dirs = Vec::with_capacity(count);
    for i in 0..count {
        let d = base.join(format!("_bench_{i:04}"));
        fs::create_dir_all(&d).with_context(|| d.display().to_string())?;
        dirs.push(d);
    }
    Ok(dirs)
}

/// Build the set of benchmark workloads.
fn create_workloads(
    root: &Path,
    git_bin: &Path,
    iterations: usize,
) -> anyhow::Result<Vec<(String, Workload)>> {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).with_context(|| workspace.display().to_string())?;

    let outside = workspace.join("outside");
    fs::create_dir_all(&outside).with_context(|| outside.display().to_string())?;

    let repo_small = workspace.join("repo-small");
    create_repo(&repo_small, git_bin, 24)?;

    let repo_medium = workspace.join("repo-medium");
    create_repo(&repo_medium, git_bin, 240)?;

    let repo_toolchain = workspace.join("repo-toolchain");
    create_toolchain_repo(&repo_toolchain, git_bin)?;

    let subdir_count = iterations + 2;
    Ok(vec![
        (
            "outside".to_owned(),
            Workload {
                path: outside.clone(),
                description: "Non-repository directory".to_owned(),
                subdirs: create_subdirs(&outside, subdir_count)?,
            },
        ),
        (
            "repo-small".to_owned(),
            Workload {
                path: repo_small.clone(),
                description: "Small clean Git repository (24 files)".to_owned(),
                subdirs: create_subdirs(&repo_small, subdir_count)?,
            },
        ),
        (
            "repo-medium".to_owned(),
            Workload {
                path: repo_medium.clone(),
                description: "Medium clean Git repository (240 files)".to_owned(),
                subdirs: create_subdirs(&repo_medium, subdir_count)?,
            },
        ),
        (
            "repo-toolchain".to_owned(),
            Workload {
                path: repo_toolchain.clone(),
                description: "Git repository with Cargo.toml (toolchain detection)".to_owned(),
                subdirs: create_subdirs(&repo_toolchain, subdir_count)?,
            },
        ),
    ])
}

fn write_bench_config(home_dir: &Path) -> io::Result<()> {
    const CONFIG: &str = r#"[[module]]
name = "rust"
icon = "🦀"
when = { files = ["Cargo.toml"] }

[[module.source]]
command = ["rustc", "--version"]
regex = "rustc (\S+)"
"#;
    fs::write(home_dir.join(".capsule").join("config.toml"), CONFIG)
}

#[cfg(unix)]
fn start_daemon(capsule_bin: &Path, home_dir: &Path) -> anyhow::Result<Child> {
    use std::os::unix::net::UnixStream;

    let home = OsString::from(home_dir.as_os_str());
    let mut child = Command::new(capsule_bin)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("HOME", home)
        .spawn()
        .with_context(|| format!("spawn {}", capsule_bin.display()))?;

    let sock_path = home_dir.join(".capsule").join("capsule.sock");
    for _ in 0..200 {
        std::thread::sleep(Duration::from_millis(10));
        if sock_path.exists() && UnixStream::connect(&sock_path).is_ok() {
            return Ok(child);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("daemon failed to start within 2s");
}

#[cfg(unix)]
fn stop_daemon(child: &mut Child) {
    let pid = child.id();
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    for _ in 0..30 {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
struct CapsuleSample {
    fast_ms: f64,
    total_ms: f64,
}

#[cfg(unix)]
struct CapsuleConn {
    read: BufReader<std::os::unix::net::UnixStream>,
    write: std::os::unix::net::UnixStream,
    path_env: String,
}

#[cfg(unix)]
impl CapsuleConn {
    fn connect(sock_path: &Path, path_env: String) -> anyhow::Result<Self> {
        use std::os::unix::net::UnixStream;

        let stream = UnixStream::connect(sock_path)
            .with_context(|| format!("connect {}", sock_path.display()))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(RENDER_RESULT_WAIT_SECS)))
            .context("set read timeout")?;
        let write = stream
            .try_clone()
            .context("clone unix stream for writing")?;
        Ok(Self {
            read: BufReader::new(stream),
            write,
            path_env,
        })
    }

    fn measure(&mut self, cwd: &Path) -> anyhow::Result<CapsuleSample> {
        let generation = CAPSULE_PROMPT_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        let session_id = SessionId::from_hex(b"deadbeefcafebabe").context("bench session id")?;
        let req = Request {
            version: PROTOCOL_VERSION,
            session_id,
            generation: PromptGeneration::new(generation),
            cwd: cwd.display().to_string(),
            cols: 120,
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main".into(),
            env_vars: vec![("PATH".into(), self.path_env.clone())],
        };
        let mut wire = req.to_wire();
        wire.push(b'\n');

        self.read
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(RENDER_RESULT_WAIT_SECS)))
            .context("set read timeout for RenderResult")?;

        let start = Instant::now();
        self.write
            .write_all(&wire)
            .context("send request to daemon")?;
        self.write.flush().context("flush daemon socket")?;

        let mut line = Vec::new();
        self.read
            .read_until(b'\n', &mut line)
            .context("read RenderResult line")?;
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        let fast_ms = start.elapsed().as_secs_f64() * 1000.0;
        let msg = Message::from_wire(&line).context("parse RenderResult")?;
        if !matches!(msg, Message::RenderResult(_)) {
            anyhow::bail!("expected RenderResult, got {msg:?}");
        }

        self.read
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(UPDATE_WAIT_MS)))
            .context("set read timeout for Update")?;

        let mut total_ms = fast_ms;
        line.clear();
        match self.read.read_until(b'\n', &mut line) {
            Ok(0) => {}
            Ok(_) => {
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                if let Ok(msg) = Message::from_wire(&line)
                    && matches!(msg, Message::Update(_))
                {
                    total_ms = start.elapsed().as_secs_f64() * 1000.0;
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
            }
            Err(e) => return Err(e).context("read Update line"),
        }

        Ok(CapsuleSample { fast_ms, total_ms })
    }
}

fn rustc_path() -> Option<PathBuf> {
    resolve_binary(Path::new("rustc"), "rustc").ok()
}

fn measure_starship(starship_bin: &Path, cwd: &Path) -> anyhow::Result<f64> {
    let start = Instant::now();
    let _ = Command::new(starship_bin)
        .args(["prompt", "--status=0", "--cmd-duration=0"])
        .current_dir(cwd)
        .env("STARSHIP_SHELL", "zsh")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("run {}", starship_bin.display()))?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn run_benchmark(
    workloads: &[(String, Workload)],
    capsule_bin: &Path,
    starship_bin: &Path,
    git_bin: &Path,
    home_dir: &Path,
    iterations: usize,
) -> anyhow::Result<Vec<ScenarioResult>> {
    let mut results = Vec::new();
    let total = workloads.len() * 2;
    let sock_path = home_dir.join(".capsule").join("capsule.sock");
    let path_env = build_path_env(capsule_bin, starship_bin, git_bin, rustc_path().as_deref());

    let mut step = 0usize;
    for (name, workload) in workloads {
        step += 1;
        eprintln!("[{step}/{total}] capsule {name}");

        let mut conn = CapsuleConn::connect(&sock_path, path_env.clone())?;
        let mut subdir_idx = 0usize;

        for _ in 0..2 {
            conn.measure(&workload.subdirs[subdir_idx])
                .with_context(|| format!("capsule warm-up {name}"))?;
            subdir_idx += 1;
        }

        let mut capsule_samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            capsule_samples.push(
                conn.measure(&workload.subdirs[subdir_idx])
                    .with_context(|| format!("capsule sample {name}"))?,
            );
            subdir_idx += 1;
        }

        let fast_values: Vec<f64> = capsule_samples.iter().map(|s| s.fast_ms).collect();
        let total_values: Vec<f64> = capsule_samples.iter().map(|s| s.total_ms).collect();
        let has_slow = capsule_samples
            .iter()
            .any(|s| (s.total_ms - s.fast_ms).abs() > f64::EPSILON);
        results.push(ScenarioResult {
            tool: "capsule".to_owned(),
            workload: name.clone(),
            description: workload.description.clone(),
            fast: summarize(&fast_values),
            slow: if has_slow {
                Some(summarize(&total_values))
            } else {
                None
            },
        });
    }

    for (name, workload) in workloads {
        step += 1;
        eprintln!("[{step}/{total}] starship {name}");

        for _ in 0..2 {
            measure_starship(starship_bin, &workload.path)
                .with_context(|| format!("starship warm-up {name}"))?;
        }

        let mut starship_values = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            starship_values.push(
                measure_starship(starship_bin, &workload.path)
                    .with_context(|| format!("starship sample {name}"))?,
            );
        }

        results.push(ScenarioResult {
            tool: "starship".to_owned(),
            workload: name.clone(),
            description: workload.description.clone(),
            fast: summarize(&starship_values),
            slow: None,
        });
    }

    results.sort_by(|a, b| (&a.workload, &a.tool).cmp(&(&b.workload, &b.tool)));

    Ok(results)
}
