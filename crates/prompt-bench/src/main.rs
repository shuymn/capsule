//! Measure prompt responses only after verifying the requested local workload ran.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

mod worker;

use std::{
    ffi::OsString,
    fmt::Write as _,
    fs,
    io::{self, Read, Seek},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use anyhow::Context;
use capsule_prompt_bench::{
    ACQUISITION_WAIT_SECS, DEFAULT_ITERATIONS, RunMetadata, ScenarioResult, build_path_env,
    resolve_binary, summarize,
};
use capsule_protocol::session::{Request, Snapshot};
use clap::Parser;
use serde::Serialize;

use self::worker::Worker;

#[derive(Parser, Debug)]
#[command(
    about = "Verify capsule and starship acquisition workloads and report diagnostic durations."
)]
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
    subdirs: Vec<PathBuf>,
    toolchain: bool,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    metadata: &'a RunMetadata,
    results: &'a [ScenarioResult],
}

fn main() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(unix_main())
    }
    #[cfg(not(unix))]
    {
        eprintln!("prompt-bench: unix-only (requires the capsule session worker)");
        std::process::exit(2);
    }
}

#[cfg(unix)]
async fn unix_main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.iterations < 1 {
        anyhow::bail!("--iterations must be at least 1");
    }

    let capsule_bin =
        resolve_binary(&args.capsule_bin, "capsule").context("resolve capsule binary")?;
    let starship_bin =
        resolve_binary(&args.starship_bin, "starship").context("resolve starship binary")?;
    let git_bin = resolve_binary(&args.git_bin, "git").context("resolve git binary")?;
    let rustc_bin = resolve_binary(Path::new("rustc"), "rustc").context("resolve rustc binary")?;

    let temp = tempfile::Builder::new()
        .prefix("prompt-bench-")
        .tempdir()
        .context("create temp dir")?;
    let root = temp.path();
    let home_dir = root.join("home");
    fs::create_dir_all(&home_dir).context("create fake HOME")?;

    let workloads =
        create_workloads(root, &git_bin, args.iterations).context("create benchmark workloads")?;

    write_bench_config(&home_dir).context("write bench config")?;
    let probe =
        ToolchainProbe::create(root, &rustc_bin).context("create rustc acquisition probe")?;
    let path_env = format!(
        "{}:{}",
        probe.bin_dir.display(),
        build_path_env(&capsule_bin, &starship_bin, &git_bin, Some(&rustc_bin))
    );

    let environment = BenchmarkEnvironment {
        home_dir: &home_dir,
        probe: &probe,
        path_env: &path_env,
    };
    let mut worker = Worker::spawn(&capsule_bin, &environment)?;
    let result = run_benchmark(
        &mut worker,
        &workloads,
        &starship_bin,
        &environment,
        args.iterations,
    )
    .await;
    let shutdown = worker.shutdown().await;
    let results = result?;
    shutdown?;

    let rustc = probe.version;
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
        "capsule: persistent session worker (initial = first matching response; completed = explicit complete=1)."
            .to_owned(),
        "starship: `starship prompt` subprocess with an isolated explicit configuration.".to_owned(),
        "These paths do not measure interactive zsh input latency. No cross-tool speedup is inferred."
            .to_owned(),
        "Every sample requires completion; same-generation redraws verify zero new rustc acquisitions."
            .to_owned(),
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
        "| Workload | Tool | initial p50 / completed p50 ms | initial p95 / completed p95 ms | verified rustc calls |".to_owned(),
        "| --- | --- | ---: | ---: | ---: |".to_owned(),
    ];

    for r in results {
        let p50 = r.slow.as_ref().map_or_else(
            || format!("{} / —", format_ms(r.fast.p50_ms)),
            |s| format!("{} / {}", format_ms(r.fast.p50_ms), format_ms(s.p50_ms)),
        );
        let p95 = r.slow.as_ref().map_or_else(
            || format!("{} / —", format_ms(r.fast.p95_ms)),
            |s| format!("{} / {}", format_ms(r.fast.p95_ms), format_ms(s.p95_ms)),
        );
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            r.workload, r.tool, p50, p95, r.toolchain_acquisitions,
        ));
    }

    lines.join("\n") + "\n"
}

fn try_command_output(argv: &[&str]) -> String {
    if argv.is_empty() {
        return "unknown".to_owned();
    }
    let Ok((out, _)) = bounded_output(Command::new(argv[0]).args(&argv[1..])) else {
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
    let (output, _) = bounded_output(
        Command::new(cmd[0])
            .args(&cmd[1..])
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null"),
    )
    .with_context(|| format!("run {}", cmd.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "command failed: {}: {}",
            cmd.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn create_toolchain_repo(repo: &Path, git_bin: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(repo).with_context(|| repo.display().to_string())?;
    let git = git_bin.to_str().context("git path is not valid UTF-8")?;
    run_command(&[git, "init", "-q"], repo)?;
    run_command(&[git, "config", "user.name", "Prompt Bench"], repo)?;
    run_command(
        &[git, "config", "user.email", "bench@example.invalid"],
        repo,
    )?;
    run_command(&[git, "config", "commit.gpgsign", "false"], repo)?;

    for index in 0..16_usize {
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

    fs::write(repo.join("Cargo.toml"), TOOLCHAIN_MANIFEST)?;
    fs::write(repo.join(".gitignore"), "_bench_*/\n")?;
    fs::write(
        repo.join("src").join("main.rs"),
        "fn main() {\n    println!(\"toolchain marker\");\n}\n",
    )?;

    run_command(&[git, "add", "."], repo)?;
    run_command(&[git, "commit", "-qm", "initial"], repo)?;
    Ok(())
}

const TOOLCHAIN_MANIFEST: &str =
    "[package]\nname = \"prompt-bench-toolchain\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

fn create_subdirs(base: &Path, count: usize, toolchain: bool) -> anyhow::Result<Vec<PathBuf>> {
    let mut dirs = Vec::with_capacity(count);
    for i in 0..count {
        let d = base.join(format!("_bench_{i:04}"));
        fs::create_dir_all(&d).with_context(|| d.display().to_string())?;
        if toolchain {
            fs::write(d.join("Cargo.toml"), TOOLCHAIN_MANIFEST)?;
        }
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

    let repo_toolchain = workspace.join("repo-toolchain");
    create_toolchain_repo(&repo_toolchain, git_bin)?;

    let subdir_count = iterations + 2;
    Ok(vec![
        (
            "outside".to_owned(),
            Workload {
                path: outside.clone(),
                subdirs: create_subdirs(&outside, subdir_count, false)?,
                toolchain: false,
            },
        ),
        (
            "repo-toolchain".to_owned(),
            Workload {
                path: repo_toolchain.clone(),
                subdirs: create_subdirs(&repo_toolchain, subdir_count, true)?,
                toolchain: true,
            },
        ),
    ])
}

fn write_bench_config(home_dir: &Path) -> io::Result<()> {
    let config_dir = home_dir.join(".config/capsule");
    fs::create_dir_all(&config_dir)?;
    fs::write(config_dir.join("config.toml"), CAPSULE_CONFIG)?;
    fs::write(home_dir.join("starship.toml"), STARSHIP_CONFIG)
}

const CAPSULE_CONFIG: &str = r#"schema_version = 2
[[module]]
name = "rust"
when = { files = ["Cargo.toml"] }
format = "{version}"

[module.values]
version = [{ command = ["rustc", "--version"] }]
"#;

const STARSHIP_CONFIG: &str = r#"format = '$directory$git_branch$git_status${custom.rust}$character'
add_newline = false
command_timeout = 5000

[custom.rust]
detect_files = ["Cargo.toml"]
command = "rustc --version"
format = '$output '
shell = ["sh"]
"#;

struct BenchmarkEnvironment<'a> {
    home_dir: &'a Path,
    probe: &'a ToolchainProbe,
    path_env: &'a str,
}

impl BenchmarkEnvironment<'_> {
    fn snapshot(&self, cwd: &Path) -> Snapshot {
        Snapshot {
            cwd: cwd.to_path_buf(),
            env: vec![
                (OsString::from("HOME"), self.home_dir.as_os_str().to_owned()),
                (
                    OsString::from("XDG_CONFIG_HOME"),
                    self.home_dir.join(".config").into_os_string(),
                ),
                (OsString::from("PATH"), OsString::from(self.path_env)),
                (OsString::from("GIT_CONFIG_NOSYSTEM"), OsString::from("1")),
                (
                    OsString::from("GIT_CONFIG_GLOBAL"),
                    OsString::from("/dev/null"),
                ),
            ],
        }
    }
}

struct ToolchainProbe {
    bin_dir: PathBuf,
    log_path: PathBuf,
    version: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AcquisitionCounts {
    started: usize,
    completed: usize,
    failed: usize,
}

impl ToolchainProbe {
    fn create(root: &Path, rustc_bin: &Path) -> anyhow::Result<Self> {
        let (output, _) =
            bounded_output(Command::new(rustc_bin).arg("--version").current_dir(root))?;
        anyhow::ensure!(output.status.success(), "rustc --version failed");
        let version = String::from_utf8(output.stdout)?.trim().to_owned();
        anyhow::ensure!(
            version.starts_with("rustc "),
            "unexpected rustc version: {version}"
        );

        let bin_dir = root.join("probe-bin");
        fs::create_dir_all(&bin_dir)?;
        let log_path = root.join("rustc-acquisitions.log");
        fs::write(&log_path, "")?;
        let rustup_home = std::env::var_os("RUSTUP_HOME").or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".rustup").into_os_string())
        });
        let mut rustup_env = rustup_home.map_or_else(String::new, |path| {
            format!("RUSTUP_HOME={} ", shell_quote(&path.to_string_lossy()))
        });
        if let Some(toolchain) = std::env::var_os("RUSTUP_TOOLCHAIN") {
            let _ = write!(
                rustup_env,
                "RUSTUP_TOOLCHAIN={} ",
                shell_quote(&toolchain.to_string_lossy())
            );
        }
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' start >> {log} || exit 125\n{rustup_env}{rustc} \"$@\"\nstatus=$?\nprintf 'finish:%s\\n' \"$status\" >> {log} || exit 125\nexit \"$status\"\n",
            log = shell_quote(&log_path.to_string_lossy()),
            rustc = shell_quote(&rustc_bin.to_string_lossy()),
        );
        let wrapper = bin_dir.join("rustc");
        fs::write(&wrapper, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
        }
        Ok(Self {
            bin_dir,
            log_path,
            version,
        })
    }

    fn counts(&self) -> anyhow::Result<AcquisitionCounts> {
        let mut counts = AcquisitionCounts::default();
        for line in fs::read_to_string(&self.log_path)?.lines() {
            match line {
                "start" => counts.started += 1,
                "finish:0" => counts.completed += 1,
                line if line.starts_with("finish:") => {
                    counts.completed += 1;
                    counts.failed += 1;
                }
                _ => anyhow::bail!("invalid acquisition record: {line}"),
            }
        }
        Ok(counts)
    }

    fn verify(&self, before: AcquisitionCounts, expected: usize) -> anyhow::Result<()> {
        let after = self.counts()?;
        anyhow::ensure!(
            after.started == before.started + expected
                && after.completed == before.completed + expected
                && after.failed == before.failed,
            "rustc acquisition mismatch: expected {expected} successful calls, before {before:?}, after {after:?}"
        );
        Ok(())
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn measure_starship(
    starship_bin: &Path,
    cwd: &Path,
    environment: &BenchmarkEnvironment<'_>,
    toolchain: bool,
) -> anyhow::Result<f64> {
    let before = environment.probe.counts()?;
    let (output, elapsed) = bounded_output(
        Command::new(starship_bin)
            .args([
                "prompt",
                "--status=0",
                "--cmd-duration=0",
                "--terminal-width=240",
            ])
            .current_dir(cwd)
            .env("STARSHIP_SHELL", "zsh")
            .env("HOME", environment.home_dir)
            .env("XDG_CONFIG_HOME", environment.home_dir.join(".config"))
            .env(
                "STARSHIP_CONFIG",
                environment.home_dir.join("starship.toml"),
            )
            .env("PATH", environment.path_env),
    )
    .with_context(|| format!("run {}", starship_bin.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "starship failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if toolchain {
        anyhow::ensure!(
            String::from_utf8_lossy(&output.stdout).contains(&environment.probe.version),
            "starship prompt is missing rustc output"
        );
    }
    environment.probe.verify(before, usize::from(toolchain))?;
    Ok(elapsed.as_secs_f64() * 1000.0)
}

fn bounded_output(command: &mut Command) -> anyhow::Result<(Output, Duration)> {
    bounded_output_with_timeout(command, Duration::from_secs(ACQUISITION_WAIT_SECS))
}

fn bounded_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> anyhow::Result<(Output, Duration)> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command.spawn()?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status();
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("subprocess exceeded {}s deadline", timeout.as_secs_f64());
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let elapsed = started.elapsed();
    stdout.rewind()?;
    stderr.rewind()?;
    let mut output = Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    stdout.read_to_end(&mut output.stdout)?;
    stderr.read_to_end(&mut output.stderr)?;
    Ok((output, elapsed))
}

#[cfg(unix)]
async fn run_benchmark(
    worker: &mut Worker,
    workloads: &[(String, Workload)],
    starship_bin: &Path,
    environment: &BenchmarkEnvironment<'_>,
    iterations: usize,
) -> anyhow::Result<Vec<ScenarioResult>> {
    let mut results = Vec::new();
    let mut generation = 0;
    for (name, workload) in workloads {
        eprintln!("capsule acquisition: {name}");
        let mut samples = Vec::with_capacity(iterations);
        for (index, cwd) in workload.subdirs.iter().take(iterations + 2).enumerate() {
            generation += 1;
            let sample = worker
                .measure(
                    &request(generation, cwd, environment),
                    environment.probe,
                    usize::from(workload.toolchain),
                )
                .await
                .with_context(|| format!("capsule acquisition {name}"))?;
            if index >= 2 {
                samples.push(sample);
            }
        }
        results.push(worker_result(
            "capsule acquisition",
            name,
            &samples,
            usize::from(workload.toolchain) * iterations,
        ));
    }
    for (name, workload) in workloads {
        eprintln!("capsule same-generation redraw: {name}");
        generation += 1;
        let mut current = request(generation, &workload.path, environment);
        worker
            .measure(&current, environment.probe, usize::from(workload.toolchain))
            .await
            .with_context(|| format!("capsule redraw acquisition warm-up {name}"))?;
        let mut samples = Vec::with_capacity(iterations);
        for index in 0..iterations + 2 {
            // A changed glyph distinguishes this redraw from buffered duplicate
            // complete responses for the previous request in the same generation.
            current.keymap = if index % 2 == 0 { "vicmd" } else { "main" }.into();
            current.cols = if index % 2 == 0 { 239 } else { 240 };
            let sample = worker
                .measure(&current, environment.probe, 0)
                .await
                .with_context(|| format!("capsule same-generation redraw {name}"))?;
            if index >= 2 {
                samples.push(sample);
            }
        }
        results.push(worker_result(
            "capsule same-generation redraw",
            name,
            &samples,
            0,
        ));
    }
    for (name, workload) in workloads {
        eprintln!("starship: {name}");
        let mut values = Vec::with_capacity(iterations);
        for (index, cwd) in workload.subdirs.iter().take(iterations + 2).enumerate() {
            let sample = measure_starship(starship_bin, cwd, environment, workload.toolchain)
                .with_context(|| format!("starship sample {name}"))?;
            if index >= 2 {
                values.push(sample);
            }
        }
        results.push(ScenarioResult {
            tool: "starship".to_owned(),
            workload: name.clone(),
            fast: summarize(&values),
            slow: None,
            toolchain_acquisitions: usize::from(workload.toolchain) * iterations,
        });
    }
    results.sort_by(|a, b| (&a.workload, &a.tool).cmp(&(&b.workload, &b.tool)));
    Ok(results)
}

fn request(generation: u64, cwd: &Path, environment: &BenchmarkEnvironment<'_>) -> Request {
    Request {
        generation,
        snapshot: environment.snapshot(cwd),
        cols: 240,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main".into(),
    }
}

fn worker_result(
    tool: &str,
    workload: &str,
    samples: &[worker::Sample],
    calls: usize,
) -> ScenarioResult {
    ScenarioResult {
        tool: tool.to_owned(),
        workload: workload.to_owned(),
        fast: summarize(
            &samples
                .iter()
                .map(|sample| sample.initial_ms)
                .collect::<Vec<_>>(),
        ),
        slow: Some(summarize(
            &samples
                .iter()
                .map(|sample| sample.completed_ms)
                .collect::<Vec<_>>(),
        )),
        toolchain_acquisitions: calls,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn executable(path: &Path, script: &str) -> anyhow::Result<()> {
        fs::write(path, script)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    fn probe(root: &Path) -> anyhow::Result<ToolchainProbe> {
        let rustc = root.join("real-rustc");
        executable(
            &rustc,
            "#!/bin/sh\nprintf '%s\\n' 'rustc 1.99.0 (fixture)'\n",
        )?;
        ToolchainProbe::create(root, &rustc)
    }

    #[test]
    fn schema_v2_fixture_activates_one_command_in_every_toolchain_cwd() -> anyhow::Result<()> {
        use capsule_core::plan::{ConfigPlan, FormatPart, Source};

        let capsule = ConfigPlan::parse(CAPSULE_CONFIG)?;
        assert_eq!(capsule.modules.len(), 1);
        assert_eq!(capsule.modules[0].format.0, vec![FormatPart::Value(0)]);
        assert_eq!(capsule.modules[0].values[0].name, "version");
        assert_eq!(
            capsule.modules[0].values[0].candidates[0].source,
            Source::Command(vec!["rustc".into(), "--version".into()])
        );
        let starship: toml::Value = toml::from_str(STARSHIP_CONFIG)?;
        assert!(
            starship["format"]
                .as_str()
                .is_some_and(|format| format.contains("${custom.rust}"))
        );
        assert_eq!(
            starship["custom"]["rust"]["command"].as_str(),
            Some("rustc --version")
        );
        let root = tempfile::tempdir()?;
        write_bench_config(root.path())?;
        assert_eq!(
            fs::read_to_string(root.path().join(".config/capsule/config.toml"))?,
            CAPSULE_CONFIG
        );
        for cwd in create_subdirs(&root.path().join("rust"), 3, true)? {
            assert!(cwd.join(&capsule.modules[0].when.files[0]).is_file());
        }
        for cwd in create_subdirs(&root.path().join("outside"), 3, false)? {
            assert!(!cwd.join("Cargo.toml").exists());
        }
        Ok(())
    }

    #[test]
    fn probe_rejects_skipped_failed_and_unfinished_acquisitions() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let probe = probe(root.path())?;
        let before = probe.counts()?;
        assert!(probe.verify(before, 1).is_err());
        let (output, _) =
            bounded_output(Command::new(probe.bin_dir.join("rustc")).arg("--version"))?;
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout)?.contains(&probe.version));
        probe.verify(before, 1)?;
        fs::write(&probe.log_path, "start\n")?;
        assert!(probe.verify(before, 1).is_err());
        fs::write(&probe.log_path, "start\nfinish:1\n")?;
        assert!(probe.verify(before, 1).is_err());
        Ok(())
    }

    #[test]
    fn starship_requires_success_output_and_exact_command_count() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let probe = probe(root.path())?;
        let path_env = format!("{}:/usr/bin:/bin", probe.bin_dir.display());
        let environment = BenchmarkEnvironment {
            home_dir: root.path(),
            probe: &probe,
            path_env: &path_env,
        };
        let starship = root.path().join("starship");
        executable(
            &starship,
            "#!/bin/sh\ntest \"$STARSHIP_CONFIG\" = \"$HOME/starship.toml\" || exit 1\nrustc --version\n",
        )?;
        measure_starship(&starship, root.path(), &environment, true)?;
        executable(&starship, "#!/bin/sh\nexit 42\n")?;
        assert!(measure_starship(&starship, root.path(), &environment, false).is_err());
        executable(
            &starship,
            "#!/bin/sh\nprintf '%s\\n' 'rustc 1.99.0 (fixture)'\n",
        )?;
        assert!(measure_starship(&starship, root.path(), &environment, true).is_err());
        Ok(())
    }

    #[test]
    fn subprocess_deadline_returns_an_error() {
        assert!(
            bounded_output_with_timeout(
                Command::new("/bin/sh").args(["-c", "exec sleep 30"]),
                Duration::from_millis(20)
            )
            .is_err()
        );
    }
}
