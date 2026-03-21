#!/usr/bin/env -S uv run python
"""Benchmark prompt latency for capsule and starship in isolated zsh sessions."""

from __future__ import annotations

import argparse
import json
import math
import os
import pty
import random
import select
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import textwrap
import time
from collections.abc import Sequence
from dataclasses import asdict, dataclass
from pathlib import Path

MARKER_PREFIX = "__PROMPT_BENCH_READY__"
DEFAULT_COLUMNS = 120
DEFAULT_ITERATIONS = 30
DEFAULT_IDLE_WINDOW_SECONDS = 0.05
DEFAULT_ASYNC_WINDOW_SECONDS = 1.0
READ_CHUNK_SIZE = 4096
READ_TIMEOUT_SECONDS = 10.0


@dataclass(frozen=True)
class Workload:
    """A benchmark working directory."""

    name: str
    path: Path
    category: str
    description: str


@dataclass(frozen=True)
class Sample:
    """One prompt timing sample."""

    latency_ms: float
    marker_ms: float
    async_update_ms: float | None


@dataclass(frozen=True)
class SummaryStats:
    """Summary statistics for a sample series."""

    count: int
    min_ms: float
    p50_ms: float
    p95_ms: float
    max_ms: float
    mean_ms: float
    stddev_ms: float


@dataclass(frozen=True)
class ScenarioResult:
    """Aggregated results for one tool/scenario/workload tuple."""

    tool: str
    scenario: str
    workload: str
    description: str
    latency: SummaryStats
    async_update: SummaryStats | None


@dataclass(frozen=True)
class RunMetadata:
    """Metadata emitted alongside the benchmark report."""

    seed: int
    iterations: int
    columns: int
    idle_window_ms: int
    async_window_ms: int
    zsh_bin: str
    capsule_bin: str
    starship_bin: str
    git_bin: str
    python: str
    macos: str
    kernel: str
    cpu: str


@dataclass(frozen=True)
class PromptEvent:
    """Timing data for one prompt render."""

    marker_ms: float
    stable_ms: float
    async_update_ms: float | None


class BenchmarkError(RuntimeError):
    """Raised when the benchmark harness cannot complete a measurement."""


def build_parser() -> argparse.ArgumentParser:
    """Build the CLI parser."""

    parser = argparse.ArgumentParser(
        description=(
            "Benchmark capsule and starship prompt latency in isolated zsh sessions."
        ),
    )
    parser.add_argument(
        "--capsule-bin",
        type=Path,
        default=Path("target/release/capsule"),
        help="Path to the capsule binary (default: target/release/capsule)",
    )
    parser.add_argument(
        "--starship-bin",
        default="starship",
        help="Path to the starship binary or name on PATH (default: starship)",
    )
    parser.add_argument(
        "--zsh-bin",
        default="zsh",
        help="Path to the zsh binary or name on PATH (default: zsh)",
    )
    parser.add_argument(
        "--git-bin",
        default="git",
        help="Path to the git binary or name on PATH (default: git)",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=DEFAULT_ITERATIONS,
        help=f"Samples per scenario (default: {DEFAULT_ITERATIONS})",
    )
    parser.add_argument(
        "--columns",
        type=int,
        default=DEFAULT_COLUMNS,
        help=f"Fixed terminal width for the isolated shell (default: {DEFAULT_COLUMNS})",
    )
    parser.add_argument(
        "--idle-window-ms",
        type=int,
        default=int(DEFAULT_IDLE_WINDOW_SECONDS * 1000),
        help="Idle window used to decide the prompt has stabilized",
    )
    parser.add_argument(
        "--async-window-ms",
        type=int,
        default=int(DEFAULT_ASYNC_WINDOW_SECONDS * 1000),
        help="Window used to observe async prompt updates after the initial prompt",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=0,
        help="Random seed for scenario order randomization (default: time-based)",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        help="Optional path to write a JSON report",
    )
    parser.add_argument(
        "--markdown-out",
        type=Path,
        help="Optional path to write the Markdown summary",
    )
    return parser


def resolve_binary(path_or_name: str | Path, label: str) -> str:
    """Resolve an executable path and fail early if it is unavailable."""

    candidate = shutil.which(str(path_or_name))
    if candidate is not None:
        return str(Path(candidate).resolve())

    path = Path(path_or_name)
    if path.exists():
        return str(path.resolve())

    raise BenchmarkError(f"{label} not found: {path_or_name}")


def percentile(values: Sequence[float], fraction: float) -> float:
    """Compute a percentile using linear interpolation."""

    if not values:
        raise ValueError("cannot compute a percentile of an empty series")

    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]

    position = (len(ordered) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    weight = position - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * weight


def summarize(values: Sequence[float]) -> SummaryStats:
    """Summarize a non-empty list of timing values."""

    if not values:
        raise ValueError("cannot summarize an empty series")

    mean = statistics.fmean(values)
    stddev = statistics.stdev(values) if len(values) > 1 else 0.0
    return SummaryStats(
        count=len(values),
        min_ms=min(values),
        p50_ms=percentile(values, 0.50),
        p95_ms=percentile(values, 0.95),
        max_ms=max(values),
        mean_ms=mean,
        stddev_ms=stddev,
    )


def shell_quote(value: str) -> str:
    """Return a shell-safe single-quoted string."""

    return "'" + value.replace("'", "'\"'\"'") + "'"


def render_zshenv() -> str:
    """Render a minimal .zshenv for the isolated benchmark session."""

    return textwrap.dedent(
        """\
        unsetopt GLOBAL_RCS
        export HOME="$ZDOTDIR"
        export LANG="en_US.UTF-8"
        export LC_ALL="en_US.UTF-8"
        """
    )


def render_zshrc(tool: str, capsule_bin: str, starship_bin: str) -> str:
    """Render the isolated .zshrc used by one benchmark tool."""

    if tool == "capsule":
        init_block = f'eval "$({shell_quote(capsule_bin)} init zsh)"'
    elif tool == "starship":
        init_block = f'eval "$({shell_quote(starship_bin)} init zsh)"'
    else:
        raise ValueError(f"unsupported tool: {tool}")

    return textwrap.dedent(
        f"""\
        {init_block}

        typeset -gi _PROMPT_BENCH_SEQ=0

        _prompt_bench_notify() {{
            (( _PROMPT_BENCH_SEQ++ ))
            if [[ -n "${{PROMPT_BENCH_NOTIFY_FD:-}}" ]]; then
                print -u $PROMPT_BENCH_NOTIFY_FD -- "{MARKER_PREFIX}:${{_PROMPT_BENCH_SEQ}}"
            fi
        }}

        precmd_functions+=(_prompt_bench_notify)
        """
    )


def write_text(path: Path, content: str) -> None:
    """Write UTF-8 text to a file."""

    path.write_text(content, encoding="utf-8")


def run_command(command: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> str:
    """Run a subprocess and return its stdout as UTF-8 text."""

    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


def create_repo(repo: Path, git_bin: str, file_count: int) -> None:
    """Create a deterministic Git repository for benchmark workloads."""

    repo.mkdir(parents=True, exist_ok=True)
    run_command([git_bin, "init", "-q"], cwd=repo)
    run_command([git_bin, "config", "user.name", "Prompt Bench"], cwd=repo)
    run_command([git_bin, "config", "user.email", "bench@example.invalid"], cwd=repo)
    run_command([git_bin, "config", "commit.gpgsign", "false"], cwd=repo)

    for index in range(file_count):
        nested = repo / "src" / f"group-{index % 8}" / f"file-{index:04d}.txt"
        nested.parent.mkdir(parents=True, exist_ok=True)
        nested.write_text(f"sample file {index}\n", encoding="utf-8")

    tracked = repo / "tracked.txt"
    tracked.write_text("tracked baseline\n", encoding="utf-8")
    run_command([git_bin, "add", "."], cwd=repo)
    run_command([git_bin, "commit", "-qm", "initial"], cwd=repo)


def create_toolchain_repo(repo: Path, git_bin: str) -> None:
    """Create a small Rust repository that triggers toolchain detection."""

    create_repo(repo, git_bin, file_count=16)
    cargo_toml = textwrap.dedent(
        """\
        [package]
        name = "prompt-bench-toolchain"
        version = "0.1.0"
        edition = "2024"
        """
    )
    write_text(repo / "Cargo.toml", cargo_toml)
    main_rs = textwrap.dedent(
        """\
        fn main() {
            println!("toolchain marker");
        }
        """
    )
    write_text(repo / "src" / "main.rs", main_rs)


def create_workloads(root: Path, git_bin: str) -> tuple[Path, dict[str, Workload]]:
    """Create the benchmark directories."""

    workspace = root / "workspace"
    workspace.mkdir(parents=True, exist_ok=True)

    outside = workspace / "outside"
    outside.mkdir()

    repo_small = workspace / "repo-small"
    create_repo(repo_small, git_bin, file_count=24)
    deep_dir = repo_small / "src" / "group-0" / "nested" / "deeper" / "deepest"
    deep_dir.mkdir(parents=True, exist_ok=True)
    write_text(deep_dir / ".keep", "deep workload marker\n")
    run_command([git_bin, "add", "."], cwd=repo_small)
    run_command([git_bin, "commit", "-qm", "add deep workload"], cwd=repo_small)

    repo_medium = workspace / "repo-medium"
    create_repo(repo_medium, git_bin, file_count=240)

    repo_toolchain = workspace / "repo-toolchain"
    create_toolchain_repo(repo_toolchain, git_bin)
    run_command([git_bin, "add", "."], cwd=repo_toolchain)
    run_command([git_bin, "commit", "-qm", "toolchain"], cwd=repo_toolchain)

    workloads = {
        "outside": Workload(
            name="outside",
            path=outside,
            category="non_repo",
            description="Non-repository directory",
        ),
        "repo-small": Workload(
            name="repo-small",
            path=repo_small,
            category="git_repo",
            description="Small clean Git repository",
        ),
        "repo-medium": Workload(
            name="repo-medium",
            path=repo_medium,
            category="git_repo",
            description="Medium clean Git repository",
        ),
        "repo-toolchain": Workload(
            name="repo-toolchain",
            path=repo_toolchain,
            category="toolchain_repo",
            description="Git repository with Cargo.toml for toolchain detection",
        ),
        "repo-small-deep": Workload(
            name="repo-small-deep",
            path=deep_dir,
            category="deep_subdir",
            description="Deep subdirectory inside the small Git repository",
        ),
    }
    return workspace, workloads


def build_minimal_path(executables: Sequence[str]) -> str:
    """Build a PATH that exposes only the required command directories."""

    ordered: list[str] = []
    seen: set[str] = set()
    for executable in executables:
        directory = str(Path(executable).resolve().parent)
        if directory not in seen:
            seen.add(directory)
            ordered.append(directory)
    for directory in ("/usr/bin", "/bin"):
        if directory not in seen:
            seen.add(directory)
            ordered.append(directory)
    return os.pathsep.join(ordered)


class ZshSession:
    """PTY-backed isolated interactive zsh session."""

    def __init__(
        self,
        *,
        zsh_bin: str,
        cwd: Path,
        env: dict[str, str],
        columns: int,
        idle_window_seconds: float,
        async_window_seconds: float,
    ) -> None:
        self._zsh_bin = zsh_bin
        self._cwd = cwd
        self._env = env
        self._columns = columns
        self._idle_window_seconds = idle_window_seconds
        self._async_window_seconds = async_window_seconds
        self._pid: int | None = None
        self._fd: int | None = None
        self._notify_fd: int | None = None
        self._buffer = bytearray()
        self._notify_buffer = bytearray()
        self._prompt_seq = 0

    def __enter__(self) -> "ZshSession":
        self.spawn()
        return self

    def __exit__(self, exc_type, exc, traceback) -> None:
        self.close()

    @property
    def prompt_seq(self) -> int:
        """Return the most recent prompt marker sequence number."""

        return self._prompt_seq

    def spawn(self) -> None:
        """Spawn the interactive shell."""

        if self._pid is not None or self._fd is not None:
            raise BenchmarkError("session already spawned")

        read_fd, write_fd = os.pipe()
        os.set_inheritable(write_fd, True)
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(self._cwd)
            os.close(read_fd)
            child_env = dict(self._env)
            child_env["PROMPT_BENCH_NOTIFY_FD"] = str(write_fd)
            os.execve(self._zsh_bin, [self._zsh_bin, "-d", "-i"], child_env)

        os.close(write_fd)
        self._pid = pid
        self._fd = fd
        self._notify_fd = read_fd

    def close(self) -> None:
        """Terminate the shell and release the PTY."""

        if self._fd is not None:
            try:
                os.close(self._fd)
            except OSError:
                pass
            self._fd = None

        if self._notify_fd is not None:
            try:
                os.close(self._notify_fd)
            except OSError:
                pass
            self._notify_fd = None

        if self._pid is not None:
            try:
                os.kill(self._pid, signal.SIGTERM)
            except OSError:
                pass
            try:
                os.waitpid(self._pid, 0)
            except ChildProcessError:
                pass
            self._pid = None

    def run_and_measure(self, command: str) -> PromptEvent:
        """Send a command and measure the following prompt."""

        self.write(command + "\n")
        return self.wait_for_next_prompt()

    def wait_for_next_prompt(self) -> PromptEvent:
        """Wait for the next prompt marker and compute prompt timings."""

        target_seq = self._prompt_seq + 1
        start = time.perf_counter()
        notified_time: float | None = None

        while True:
            self._drain_pty(timeout=0.01)
            notification = self._read_notification(timeout=0.01)
            if notification is not None:
                seq, notify_time = notification
                if seq == target_seq:
                    notified_time = notify_time
                    break

            if time.perf_counter() - start > READ_TIMEOUT_SECONDS:
                output = self._buffer.decode("utf-8", errors="replace")[-400:]
                raise BenchmarkError(
                    f"timed out waiting for prompt notification {target_seq}: {output}"
                )

        if notified_time is None:
            raise BenchmarkError("prompt notification missing")

        settled_time = self._wait_for_quiet_output(notified_time)
        self._prompt_seq = target_seq
        stable_ms = (settled_time - start) * 1000.0
        marker_ms = (notified_time - start) * 1000.0
        async_ms = self._observe_async_update(start=start, base_time=settled_time)
        return PromptEvent(
            marker_ms=marker_ms,
            stable_ms=stable_ms,
            async_update_ms=async_ms,
        )

    def write(self, text: str) -> None:
        """Write text to the PTY."""

        if self._fd is None:
            raise BenchmarkError("session not spawned")
        os.write(self._fd, text.encode())

    def _observe_async_update(self, *, start: float, base_time: float) -> float | None:
        """Observe the first async update after the initial prompt, if any."""

        deadline = base_time + self._async_window_seconds
        first_async_time: float | None = None

        while True:
            remaining = deadline - time.perf_counter()
            if remaining <= 0:
                break

            chunk, chunk_time = self._read_pty(deadline=time.perf_counter() + remaining)
            if not chunk:
                continue
            self._buffer.extend(chunk)
            if first_async_time is None:
                first_async_time = chunk_time

        if first_async_time is None:
            return None
        return (first_async_time - start) * 1000.0

    def _wait_for_quiet_output(self, notified_time: float) -> float:
        """Wait until PTY output is quiet for the configured idle window."""

        last_output_time = notified_time

        while True:
            chunk, chunk_time = self._read_pty(
                deadline=time.perf_counter() + self._idle_window_seconds
            )
            if chunk:
                self._buffer.extend(chunk)
                last_output_time = chunk_time
                continue

            if time.perf_counter() - last_output_time >= self._idle_window_seconds:
                return last_output_time

    def _drain_pty(self, *, timeout: float) -> None:
        """Drain PTY output opportunistically."""

        deadline = time.perf_counter() + timeout
        while True:
            chunk, _ = self._read_pty(deadline=deadline)
            if not chunk:
                return
            self._buffer.extend(chunk)

    def _read_notification(self, *, timeout: float) -> tuple[int, float] | None:
        """Read one prompt-ready notification from the side channel."""

        if self._notify_fd is None:
            raise BenchmarkError("session notification pipe is not available")

        readable, _, _ = select.select([self._notify_fd], [], [], timeout)
        if not readable:
            return None

        chunk = os.read(self._notify_fd, READ_CHUNK_SIZE)
        if not chunk:
            output = self._buffer.decode("utf-8", errors="replace")[-400:]
            raise BenchmarkError(f"zsh notify pipe reached EOF: {output}")

        self._notify_buffer.extend(chunk)
        while b"\n" in self._notify_buffer:
            line, _, remainder = self._notify_buffer.partition(b"\n")
            self._notify_buffer = bytearray(remainder)
            text = line.decode("utf-8", errors="replace").strip()
            if text.startswith(f"{MARKER_PREFIX}:"):
                _, seq_text = text.split(":", maxsplit=1)
                return int(seq_text), time.perf_counter()
        return None

    def _read_pty(self, *, deadline: float) -> tuple[bytes, float]:
        """Read one PTY chunk if available before the deadline."""

        if self._fd is None:
            raise BenchmarkError("session not spawned")

        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            return b"", time.perf_counter()

        readable, _, _ = select.select([self._fd], [], [], remaining)
        if not readable:
            return b"", time.perf_counter()

        try:
            chunk = os.read(self._fd, READ_CHUNK_SIZE)
        except OSError as exc:
            raise BenchmarkError(f"failed to read from zsh PTY: {exc}") from exc

        if not chunk:
            output = self._buffer.decode("utf-8", errors="replace")[-400:]
            raise BenchmarkError(f"zsh PTY reached EOF before prompt was ready: {output}")
        return chunk, time.perf_counter()


def create_isolated_env(
    *,
    root: Path,
    tool: str,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    columns: int,
    term: str,
) -> dict[str, str]:
    """Create an isolated HOME/ZDOTDIR/TMPDIR tree and environment."""

    sessions_root = root / "sessions"
    sessions_root.mkdir(parents=True, exist_ok=True)
    env_root = Path(
        tempfile.mkdtemp(prefix=f"{tool}-", dir=sessions_root)
    )
    zdotdir = env_root / "zdotdir"
    tmpdir = env_root / "tmp"
    zdotdir.mkdir(parents=True, exist_ok=False)
    tmpdir.mkdir(parents=True, exist_ok=False)

    write_text(zdotdir / ".zshenv", render_zshenv())
    write_text(zdotdir / ".zshrc", render_zshrc(tool, capsule_bin, starship_bin))

    path = build_minimal_path([zsh_bin, capsule_bin, starship_bin, git_bin])
    return {
        "HOME": str(zdotdir),
        "ZDOTDIR": str(zdotdir),
        "TMPDIR": str(tmpdir),
        "PATH": path,
        "TERM": term,
        "COLUMNS": str(columns),
    }


def collect_metadata(
    *,
    seed: int,
    iterations: int,
    columns: int,
    idle_window_ms: int,
    async_window_ms: int,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
) -> RunMetadata:
    """Collect runtime metadata for the benchmark report."""

    def try_command(command: list[str]) -> str:
        try:
            return run_command(command)
        except (OSError, subprocess.CalledProcessError):
            return "unknown"

    return RunMetadata(
        seed=seed,
        iterations=iterations,
        columns=columns,
        idle_window_ms=idle_window_ms,
        async_window_ms=async_window_ms,
        zsh_bin=zsh_bin,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
        python=sys.version.split()[0],
        macos=try_command(["sw_vers", "-productVersion"]),
        kernel=try_command(["uname", "-srv"]),
        cpu=try_command(["sysctl", "-n", "machdep.cpu.brand_string"]),
    )


def measure_cold_start(
    *,
    tool: str,
    workload: Workload,
    root: Path,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    iterations: int,
    columns: int,
    idle_window_seconds: float,
    async_window_seconds: float,
    term: str,
) -> list[Sample]:
    """Measure initial shell startup to first prompt."""

    samples: list[Sample] = []
    for _ in range(iterations):
        env = create_isolated_env(
            root=root,
            tool=tool,
            zsh_bin=zsh_bin,
            capsule_bin=capsule_bin,
            starship_bin=starship_bin,
            git_bin=git_bin,
            columns=columns,
            term=term,
        )
        session = ZshSession(
            zsh_bin=zsh_bin,
            cwd=workload.path,
            env=env,
            columns=columns,
            idle_window_seconds=idle_window_seconds,
            async_window_seconds=async_window_seconds,
        )
        with session:
            event = session.wait_for_next_prompt()
        samples.append(
            Sample(
                latency_ms=event.stable_ms,
                marker_ms=event.marker_ms,
                async_update_ms=event.async_update_ms,
            )
        )
    return samples


def measure_steady_state(
    *,
    tool: str,
    workload: Workload,
    root: Path,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    iterations: int,
    columns: int,
    idle_window_seconds: float,
    async_window_seconds: float,
    term: str,
) -> list[Sample]:
    """Measure prompt latency after a trivial command in one warm shell."""

    env = create_isolated_env(
        root=root,
        tool=tool,
        zsh_bin=zsh_bin,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
        columns=columns,
        term=term,
    )
    samples: list[Sample] = []
    with ZshSession(
        zsh_bin=zsh_bin,
        cwd=workload.path,
        env=env,
        columns=columns,
        idle_window_seconds=idle_window_seconds,
        async_window_seconds=async_window_seconds,
    ) as session:
        session.wait_for_next_prompt()
        for _ in range(iterations):
            event = session.run_and_measure("true")
            samples.append(
                Sample(
                    latency_ms=event.stable_ms,
                    marker_ms=event.marker_ms,
                    async_update_ms=event.async_update_ms,
                )
            )
    return samples


def measure_cd_change(
    *,
    tool: str,
    workload: Workload,
    root: Path,
    workspace_root: Path,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    iterations: int,
    columns: int,
    idle_window_seconds: float,
    async_window_seconds: float,
    term: str,
) -> list[Sample]:
    """Measure prompt latency after changing directories."""

    env = create_isolated_env(
        root=root,
        tool=tool,
        zsh_bin=zsh_bin,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
        columns=columns,
        term=term,
    )
    samples: list[Sample] = []
    quoted_target = shell_quote(str(workload.path))
    quoted_root = shell_quote(str(workspace_root))
    with ZshSession(
        zsh_bin=zsh_bin,
        cwd=workspace_root,
        env=env,
        columns=columns,
        idle_window_seconds=idle_window_seconds,
        async_window_seconds=async_window_seconds,
    ) as session:
        session.wait_for_next_prompt()
        for _ in range(iterations):
            session.run_and_measure(f"cd {quoted_root}")
            event = session.run_and_measure(f"cd {quoted_target}")
            samples.append(
                Sample(
                    latency_ms=event.stable_ms,
                    marker_ms=event.marker_ms,
                    async_update_ms=event.async_update_ms,
                )
            )
    return samples


def measure_sleep_duration(
    *,
    tool: str,
    workload: Workload,
    root: Path,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    iterations: int,
    columns: int,
    idle_window_seconds: float,
    async_window_seconds: float,
    term: str,
) -> list[Sample]:
    """Measure prompt latency after a slow command."""

    env = create_isolated_env(
        root=root,
        tool=tool,
        zsh_bin=zsh_bin,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
        columns=columns,
        term=term,
    )
    samples: list[Sample] = []
    with ZshSession(
        zsh_bin=zsh_bin,
        cwd=workload.path,
        env=env,
        columns=columns,
        idle_window_seconds=idle_window_seconds,
        async_window_seconds=async_window_seconds,
    ) as session:
        session.wait_for_next_prompt()
        for _ in range(iterations):
            event = session.run_and_measure("sleep 3")
            samples.append(
                Sample(
                    latency_ms=event.stable_ms,
                    marker_ms=event.marker_ms,
                    async_update_ms=event.async_update_ms,
                )
            )
    return samples


def measure_git_change(
    *,
    tool: str,
    root: Path,
    git_bin: str,
    change_kind: str,
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    iterations: int,
    columns: int,
    idle_window_seconds: float,
    async_window_seconds: float,
    term: str,
) -> list[Sample]:
    """Measure prompt latency after a Git state change."""

    repo = root / "workspace" / "repo-small"
    env = create_isolated_env(
        root=root,
        tool=tool,
        zsh_bin=zsh_bin,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
        columns=columns,
        term=term,
    )
    samples: list[Sample] = []
    tracked_path = shell_quote(str(repo / "tracked.txt"))
    untracked_path = shell_quote(str(repo / "scratch.tmp"))

    with ZshSession(
        zsh_bin=zsh_bin,
        cwd=repo,
        env=env,
        columns=columns,
        idle_window_seconds=idle_window_seconds,
        async_window_seconds=async_window_seconds,
    ) as session:
        session.wait_for_next_prompt()
        for _ in range(iterations):
            run_command([git_bin, "reset", "--hard", "-q", "HEAD"], cwd=repo)
            run_command([git_bin, "clean", "-fdq"], cwd=repo)
            session.run_and_measure(
                f"{shell_quote(git_bin)} reset --hard -q HEAD"
            )
            if change_kind == "untracked":
                event = session.run_and_measure(f"touch {untracked_path}")
            elif change_kind == "dirty":
                event = session.run_and_measure(
                    f"printf '%s\\n' bench-change >> {tracked_path}"
                )
            else:
                raise ValueError(f"unsupported git change kind: {change_kind}")
            samples.append(
                Sample(
                    latency_ms=event.stable_ms,
                    marker_ms=event.marker_ms,
                    async_update_ms=event.async_update_ms,
                )
            )
    return samples


def benchmark(
    *,
    root: Path,
    workspace_root: Path,
    workloads: dict[str, Workload],
    zsh_bin: str,
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    iterations: int,
    columns: int,
    idle_window_seconds: float,
    async_window_seconds: float,
    seed: int,
    term: str,
) -> list[ScenarioResult]:
    """Run the full benchmark matrix."""

    randomizer = random.Random(seed)
    plans: list[tuple[str, str, str, str]] = []
    for tool in ("capsule", "starship"):
        for workload_name in workloads:
            plans.append((tool, "cold_start", workload_name, workloads[workload_name].description))
            plans.append((tool, "warm_steady_state", workload_name, workloads[workload_name].description))
            plans.append((tool, "context_change_cd", workload_name, workloads[workload_name].description))
            plans.append((tool, "cmd_duration_sleep", workload_name, workloads[workload_name].description))
        plans.append((tool, "git_state_untracked", "repo-small", "Small repo after creating an untracked file"))
        plans.append((tool, "git_state_dirty", "repo-small", "Small repo after modifying a tracked file"))
    randomizer.shuffle(plans)

    results: list[ScenarioResult] = []
    for tool, scenario, workload_name, description in plans:
        workload = workloads[workload_name]
        if scenario == "cold_start":
            samples = measure_cold_start(
                tool=tool,
                workload=workload,
                root=root,
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                git_bin=git_bin,
                iterations=iterations,
                columns=columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                term=term,
            )
        elif scenario == "warm_steady_state":
            samples = measure_steady_state(
                tool=tool,
                workload=workload,
                root=root,
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                git_bin=git_bin,
                iterations=iterations,
                columns=columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                term=term,
            )
        elif scenario == "context_change_cd":
            samples = measure_cd_change(
                tool=tool,
                workload=workload,
                root=root,
                workspace_root=workspace_root,
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                git_bin=git_bin,
                iterations=iterations,
                columns=columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                term=term,
            )
        elif scenario == "cmd_duration_sleep":
            samples = measure_sleep_duration(
                tool=tool,
                workload=workload,
                root=root,
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                git_bin=git_bin,
                iterations=iterations,
                columns=columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                term=term,
            )
        elif scenario == "git_state_untracked":
            samples = measure_git_change(
                tool=tool,
                root=root,
                git_bin=git_bin,
                change_kind="untracked",
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                iterations=iterations,
                columns=columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                term=term,
            )
        elif scenario == "git_state_dirty":
            samples = measure_git_change(
                tool=tool,
                root=root,
                git_bin=git_bin,
                change_kind="dirty",
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                iterations=iterations,
                columns=columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                term=term,
            )
        else:
            raise ValueError(f"unknown scenario: {scenario}")

        async_values = [sample.async_update_ms for sample in samples if sample.async_update_ms is not None]
        results.append(
            ScenarioResult(
                tool=tool,
                scenario=scenario,
                workload=workload_name,
                description=description,
                latency=summarize([sample.latency_ms for sample in samples]),
                async_update=summarize(async_values) if async_values else None,
            )
        )
    return sorted(results, key=lambda item: (item.scenario, item.workload, item.tool))


def format_ms(value: float) -> str:
    """Format milliseconds for Markdown output."""

    return f"{value:.2f}"


def render_markdown(metadata: RunMetadata, results: Sequence[ScenarioResult]) -> str:
    """Render a Markdown summary."""

    lines = [
        "# Prompt Benchmark Report",
        "",
        "This report compares `capsule` and `starship` under isolated `zsh` sessions.",
        "It is a standard-configuration comparison, not a feature-parity microbenchmark.",
        "",
        "## Environment",
        "",
        f"- Seed: `{metadata.seed}`",
        f"- Iterations per scenario: `{metadata.iterations}`",
        f"- Terminal columns: `{metadata.columns}`",
        f"- Idle window: `{metadata.idle_window_ms} ms`",
        f"- Async observation window: `{metadata.async_window_ms} ms`",
        f"- macOS: `{metadata.macos}`",
        f"- Kernel: `{metadata.kernel}`",
        f"- CPU: `{metadata.cpu}`",
        f"- zsh: `{metadata.zsh_bin}`",
        f"- capsule: `{metadata.capsule_bin}`",
        f"- starship: `{metadata.starship_bin}`",
        "",
        "## Results",
        "",
        "| Scenario | Workload | Tool | p50 ms | p95 ms | max ms | stddev ms | Async p50 ms | Notes |",
        "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |",
    ]

    for result in results:
        async_p50 = (
            format_ms(result.async_update.p50_ms)
            if result.async_update is not None
            else "-"
        )
        lines.append(
            "| "
            + " | ".join(
                [
                    result.scenario,
                    result.workload,
                    result.tool,
                    format_ms(result.latency.p50_ms),
                    format_ms(result.latency.p95_ms),
                    format_ms(result.latency.max_ms),
                    format_ms(result.latency.stddev_ms),
                    async_p50,
                    result.description,
                ]
            )
            + " |"
        )

    return "\n".join(lines) + "\n"


def main() -> int:
    """CLI entry point."""

    parser = build_parser()
    args = parser.parse_args()

    if args.iterations < 1:
        parser.error("--iterations must be at least 1")
    if args.columns < 20:
        parser.error("--columns must be at least 20")
    if args.idle_window_ms < 1:
        parser.error("--idle-window-ms must be positive")
    if args.async_window_ms < 0:
        parser.error("--async-window-ms must not be negative")

    seed = args.seed or int(time.time())

    try:
        zsh_bin = resolve_binary(args.zsh_bin, "zsh")
        capsule_bin = resolve_binary(args.capsule_bin, "capsule")
        starship_bin = resolve_binary(args.starship_bin, "starship")
        git_bin = resolve_binary(args.git_bin, "git")
    except BenchmarkError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    term = os.environ.get("TERM", "xterm-256color")
    idle_window_seconds = args.idle_window_ms / 1000.0
    async_window_seconds = args.async_window_ms / 1000.0

    with tempfile.TemporaryDirectory(prefix="prompt-bench-") as temp_root:
        root = Path(temp_root)
        workspace_root, workloads = create_workloads(root, git_bin)
        try:
            results = benchmark(
                root=root,
                workspace_root=workspace_root,
                workloads=workloads,
                zsh_bin=zsh_bin,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                git_bin=git_bin,
                iterations=args.iterations,
                columns=args.columns,
                idle_window_seconds=idle_window_seconds,
                async_window_seconds=async_window_seconds,
                seed=seed,
                term=term,
            )
        except BenchmarkError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1

    metadata = collect_metadata(
        seed=seed,
        iterations=args.iterations,
        columns=args.columns,
        idle_window_ms=args.idle_window_ms,
        async_window_ms=args.async_window_ms,
        zsh_bin=zsh_bin,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
    )

    markdown = render_markdown(metadata, results)
    print(markdown, end="")

    if args.markdown_out is not None:
        write_text(args.markdown_out, markdown)

    if args.json_out is not None:
        payload = {
            "metadata": asdict(metadata),
            "results": [asdict(result) for result in results],
        }
        write_text(args.json_out, json.dumps(payload, indent=2, sort_keys=True) + "\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
