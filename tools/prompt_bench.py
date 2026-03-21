#!/usr/bin/env -S uv run python
"""Benchmark prompt latency for capsule and starship.

Both tools are measured via subprocess invocation for fairness:
- capsule: pipes a request through `capsule connect` (daemon must be warm).
- starship: runs `starship prompt`.

Usage:
    uv run tools/prompt_bench.py [--iterations N] [--json-out path]
"""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import textwrap
import time
from collections.abc import Sequence
from dataclasses import asdict, dataclass
from pathlib import Path

DEFAULT_ITERATIONS = 30
SLOW_TIMEOUT_SECONDS = 2.0


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Workload:
    name: str
    path: Path
    description: str
    subdirs: tuple[Path, ...] = ()


@dataclass(frozen=True)
class CapsuleSample:
    """One capsule measurement with fast/slow breakdown."""

    fast_ms: float
    total_ms: float


@dataclass(frozen=True)
class SummaryStats:
    count: int
    min_ms: float
    p50_ms: float
    p95_ms: float
    max_ms: float
    mean_ms: float
    stddev_ms: float


@dataclass(frozen=True)
class ScenarioResult:
    tool: str
    workload: str
    description: str
    fast: SummaryStats
    slow: SummaryStats | None


@dataclass(frozen=True)
class RunMetadata:
    iterations: int
    capsule_bin: str
    starship_bin: str
    git_bin: str
    python: str
    macos: str
    kernel: str
    cpu: str


# ---------------------------------------------------------------------------
# Stats helpers
# ---------------------------------------------------------------------------


def percentile(values: Sequence[float], fraction: float) -> float:
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


# ---------------------------------------------------------------------------
# Binary resolution
# ---------------------------------------------------------------------------


def resolve_binary(path_or_name: str | Path, label: str) -> str:
    candidate = shutil.which(str(path_or_name))
    if candidate is not None:
        return str(Path(candidate).resolve())
    path = Path(path_or_name)
    if path.exists():
        return str(path.resolve())
    raise RuntimeError(f"{label} not found: {path_or_name}")


def run_command(
    command: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None
) -> str:
    completed = subprocess.run(
        command, cwd=cwd, env=env, check=True, capture_output=True, text=True
    )
    return completed.stdout.strip()


# ---------------------------------------------------------------------------
# Workload creation
# ---------------------------------------------------------------------------


def create_repo(repo: Path, git_bin: str, file_count: int) -> None:
    repo.mkdir(parents=True, exist_ok=True)
    run_command([git_bin, "init", "-q"], cwd=repo)
    run_command([git_bin, "config", "user.name", "Prompt Bench"], cwd=repo)
    run_command([git_bin, "config", "user.email", "bench@example.invalid"], cwd=repo)
    run_command([git_bin, "config", "commit.gpgsign", "false"], cwd=repo)
    for index in range(file_count):
        nested = repo / "src" / f"group-{index % 8}" / f"file-{index:04d}.txt"
        nested.parent.mkdir(parents=True, exist_ok=True)
        nested.write_text(f"sample file {index}\n", encoding="utf-8")
    run_command([git_bin, "add", "."], cwd=repo)
    run_command([git_bin, "commit", "-qm", "initial"], cwd=repo)


def create_toolchain_repo(repo: Path, git_bin: str) -> None:
    """Create a small Rust repository that triggers toolchain detection."""
    create_repo(repo, git_bin, file_count=16)
    (repo / "Cargo.toml").write_text(
        "[package]\nname = \"prompt-bench-toolchain\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        encoding="utf-8",
    )
    (repo / "src" / "main.rs").write_text(
        'fn main() {\n    println!("toolchain marker");\n}\n',
        encoding="utf-8",
    )
    run_command([git_bin, "add", "."], cwd=repo)
    run_command([git_bin, "commit", "-qm", "toolchain"], cwd=repo)


def create_subdirs(base: Path, count: int) -> tuple[Path, ...]:
    """Create unique subdirectories to force daemon cache misses."""
    dirs: list[Path] = []
    for i in range(count):
        d = base / f"_bench_{i:04d}"
        d.mkdir(parents=True, exist_ok=True)
        dirs.append(d)
    return tuple(dirs)


def create_workloads(root: Path, git_bin: str, iterations: int) -> dict[str, Workload]:
    workspace = root / "workspace"
    workspace.mkdir(parents=True, exist_ok=True)

    outside = workspace / "outside"
    outside.mkdir()

    repo_small = workspace / "repo-small"
    create_repo(repo_small, git_bin, file_count=24)

    repo_medium = workspace / "repo-medium"
    create_repo(repo_medium, git_bin, file_count=240)

    repo_toolchain = workspace / "repo-toolchain"
    create_toolchain_repo(repo_toolchain, git_bin)

    # +2 for warm-up rounds
    subdir_count = iterations + 2
    return {
        "outside": Workload(
            "outside",
            outside,
            "Non-repository directory",
            create_subdirs(outside, subdir_count),
        ),
        "repo-small": Workload(
            "repo-small",
            repo_small,
            "Small clean Git repository (24 files)",
            create_subdirs(repo_small, subdir_count),
        ),
        "repo-medium": Workload(
            "repo-medium",
            repo_medium,
            "Medium clean Git repository (240 files)",
            create_subdirs(repo_medium, subdir_count),
        ),
        "repo-toolchain": Workload(
            "repo-toolchain",
            repo_toolchain,
            "Git repository with Cargo.toml (toolchain detection)",
            create_subdirs(repo_toolchain, subdir_count),
        ),
    }


# ---------------------------------------------------------------------------
# Daemon lifecycle
# ---------------------------------------------------------------------------


def write_bench_config(home_dir: Path) -> None:
    """Write a capsule config with a Rust toolchain module for benchmarking."""
    config = textwrap.dedent("""\
        [[module]]
        name = "rust"
        icon = "🦀"
        when = { files = ["Cargo.toml"] }

        [[module.source]]
        command = ["rustc", "--version"]
        regex = "rustc (\\\\S+)"
    """)
    config_path = home_dir / ".capsule" / "config.toml"
    config_path.write_text(config, encoding="utf-8")


def start_daemon(
    capsule_bin: str, home_dir: Path
) -> subprocess.Popen[bytes]:
    env = dict(os.environ)
    env["HOME"] = str(home_dir)
    proc = subprocess.Popen(
        [capsule_bin, "daemon"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        env=env,
    )
    sock_path = home_dir / ".capsule" / "capsule.sock"
    for _ in range(200):
        time.sleep(0.01)
        if sock_path.exists():
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.connect(str(sock_path))
                s.close()
                return proc
            except OSError:
                pass
    proc.kill()
    raise RuntimeError("daemon failed to start within 2s")


def stop_daemon(proc: subprocess.Popen[bytes]) -> None:
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


# ---------------------------------------------------------------------------
# Netstring codec (pure Python, matching crates/protocol/)
# ---------------------------------------------------------------------------


def ns_encode(data: bytes) -> bytes:
    return f"{len(data)}:".encode() + data + b","


def ns_decode(buf: bytes, offset: int = 0) -> tuple[bytes, int]:
    colon = buf.index(b":", offset)
    length = int(buf[offset:colon])
    start = colon + 1
    end = start + length
    if buf[end : end + 1] != b",":
        raise ValueError("netstring: missing trailing comma")
    return buf[start:end], end + 1


# ---------------------------------------------------------------------------
# Capsule measurement (direct daemon socket)
# ---------------------------------------------------------------------------

SESSION_ID_HEX = b"deadbeefcafebabe"


def _build_request_wire(generation: int, cwd: str, path_env: str) -> bytes:
    """Build a netstring-encoded Request message (wire type Q)."""
    env_meta = f"PATH={path_env}".encode()
    buf = b""
    buf += ns_encode(b"1")  # version
    buf += ns_encode(b"Q")  # type
    buf += ns_encode(SESSION_ID_HEX)  # session_id
    buf += ns_encode(str(generation).encode())
    buf += ns_encode(cwd.encode())
    buf += ns_encode(b"120")  # cols
    buf += ns_encode(b"0")  # last_exit_code
    buf += ns_encode(b"")  # duration_ms (none)
    buf += ns_encode(b"main")  # keymap
    buf += ns_encode(env_meta)
    return buf + b"\n"


def _parse_msg_type(line: bytes) -> str:
    """Extract the message type tag from a wire line."""
    # Skip version field, read type field
    _, after_version = ns_decode(line)
    tag, _ = ns_decode(line, after_version)
    return tag.decode()


class CapsuleConn:
    """Persistent connection to the capsule daemon socket."""

    _global_gen: int = 0

    def __init__(self, socket_path: str, path_env: str) -> None:
        self._socket_path = socket_path
        self._path_env = path_env
        self._sock: socket.socket | None = None
        self._buf = b""

    def connect(self) -> None:
        self._sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._sock.connect(self._socket_path)
        self._sock.settimeout(SLOW_TIMEOUT_SECONDS)

    def close(self) -> None:
        if self._sock:
            self._sock.close()
            self._sock = None

    def _recv_line(self) -> bytes:
        while b"\n" not in self._buf:
            chunk = self._sock.recv(8192)  # type: ignore[union-attr]
            if not chunk:
                raise ConnectionError("daemon socket EOF")
            self._buf += chunk
        line, _, self._buf = self._buf.partition(b"\n")
        return line

    def measure(self, cwd: str) -> CapsuleSample:
        """Send Request, wait for RenderResult + Update, return latency."""
        CapsuleConn._global_gen += 1
        wire = _build_request_wire(CapsuleConn._global_gen, cwd, self._path_env)

        start = time.perf_counter()
        self._sock.sendall(wire)  # type: ignore[union-attr]

        # Wait for RenderResult (type R) — fast modules only
        line = self._recv_line()
        fast_ms = (time.perf_counter() - start) * 1000.0
        if _parse_msg_type(line) != "R":
            raise RuntimeError(f"expected R, got {_parse_msg_type(line)}")

        # Wait for Update (type U) — slow modules (git, toolchain, etc.)
        # Use a short timeout so non-repo workloads don't block.
        total_ms = fast_ms
        old_timeout = self._sock.gettimeout()  # type: ignore[union-attr]
        self._sock.settimeout(SLOW_TIMEOUT_SECONDS)  # type: ignore[union-attr]
        try:
            line = self._recv_line()
            if _parse_msg_type(line) == "U":
                total_ms = (time.perf_counter() - start) * 1000.0
        except (TimeoutError, OSError):
            pass
        finally:
            self._sock.settimeout(old_timeout)  # type: ignore[union-attr]

        return CapsuleSample(fast_ms=fast_ms, total_ms=total_ms)


# ---------------------------------------------------------------------------
# Starship measurement (subprocess: starship prompt)
# ---------------------------------------------------------------------------


def measure_starship(starship_bin: str, cwd: str) -> float:
    """Measure starship prompt via subprocess (ms)."""
    env = dict(os.environ)
    env["STARSHIP_SHELL"] = "zsh"

    start = time.perf_counter()
    subprocess.run(
        [starship_bin, "prompt", "--status=0", "--cmd-duration=0"],
        cwd=cwd,
        env=env,
        capture_output=True,
    )
    elapsed = time.perf_counter() - start

    return elapsed * 1000.0


# ---------------------------------------------------------------------------
# Benchmark runner
# ---------------------------------------------------------------------------


def benchmark(
    *,
    workloads: dict[str, Workload],
    capsule_bin: str,
    starship_bin: str,
    git_bin: str,
    iterations: int,
    home_dir: Path,
) -> list[ScenarioResult]:
    results: list[ScenarioResult] = []
    total = len(workloads) * 2
    sock_path = str(home_dir / ".capsule" / "capsule.sock")
    # Build PATH that includes all required binaries (git, rustc, etc.)
    bin_dirs: list[str] = []
    seen: set[str] = set()
    for b in [capsule_bin, starship_bin, git_bin]:
        d = str(Path(b).parent)
        if d not in seen:
            seen.add(d)
            bin_dirs.append(d)
    rustc = shutil.which("rustc")
    if rustc:
        d = str(Path(rustc).parent)
        if d not in seen:
            seen.add(d)
            bin_dirs.append(d)
    for d in ("/usr/bin", "/bin"):
        if d not in seen:
            seen.add(d)
            bin_dirs.append(d)
    path_env = os.pathsep.join(bin_dirs)

    step = 0
    for name, workload in workloads.items():
        step += 1
        print(
            f"[{step}/{total}] capsule {name}",
            file=sys.stderr,
            flush=True,
        )

        conn = CapsuleConn(sock_path, path_env)
        conn.connect()

        # Each measurement uses a unique subdir to force cache miss (cold slow path).
        subdir_idx = 0

        # Warm-up (2 rounds, discarded)
        for _ in range(2):
            conn.measure(str(workload.subdirs[subdir_idx]))
            subdir_idx += 1

        capsule_samples: list[CapsuleSample] = []
        for _ in range(iterations):
            capsule_samples.append(conn.measure(str(workload.subdirs[subdir_idx])))
            subdir_idx += 1
        conn.close()

        fast_values = [s.fast_ms for s in capsule_samples]
        total_values = [s.total_ms for s in capsule_samples]
        has_slow = any(s.total_ms != s.fast_ms for s in capsule_samples)
        results.append(
            ScenarioResult(
                tool="capsule",
                workload=name,
                description=workload.description,
                fast=summarize(fast_values),
                slow=summarize(total_values) if has_slow else None,
            )
        )

    for name, workload in workloads.items():
        step += 1
        print(
            f"[{step}/{total}] starship {name}",
            file=sys.stderr,
            flush=True,
        )

        # Warm-up
        for _ in range(2):
            measure_starship(starship_bin, str(workload.path))

        starship_values: list[float] = []
        for _ in range(iterations):
            starship_values.append(measure_starship(starship_bin, str(workload.path)))

        results.append(
            ScenarioResult(
                tool="starship",
                workload=name,
                description=workload.description,
                fast=summarize(starship_values),
                slow=None,
            )
        )

    return sorted(results, key=lambda r: (r.workload, r.tool))


# ---------------------------------------------------------------------------
# Output formatting
# ---------------------------------------------------------------------------


def format_ms(value: float) -> str:
    return f"{value:.2f}"


def render_markdown(
    metadata: RunMetadata, results: Sequence[ScenarioResult]
) -> str:
    lines = [
        "# Prompt Benchmark Report",
        "",
        "capsule: direct daemon socket (fast = RenderResult, slow = +Update with git/toolchain).",
        "starship: `starship prompt` subprocess.",
        "",
        "## Environment",
        "",
        f"- Iterations per workload: `{metadata.iterations}`",
        f"- macOS: `{metadata.macos}`",
        f"- CPU: `{metadata.cpu}`",
        f"- capsule: `{metadata.capsule_bin}`",
        f"- starship: `{metadata.starship_bin}`",
        "",
        "## Results",
        "",
        "| Workload | Tool | Fast p50 ms | Fast p95 ms | Slow p50 ms | Slow p95 ms | Description |",
        "| --- | --- | ---: | ---: | ---: | ---: | --- |",
    ]
    for r in results:
        slow_p50 = format_ms(r.slow.p50_ms) if r.slow else "-"
        slow_p95 = format_ms(r.slow.p95_ms) if r.slow else "-"
        lines.append(
            "| "
            + " | ".join([
                r.workload,
                r.tool,
                format_ms(r.fast.p50_ms),
                format_ms(r.fast.p95_ms),
                slow_p50,
                slow_p95,
                r.description,
            ])
            + " |"
        )
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Benchmark capsule and starship prompt latency.",
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
        help="Path to the starship binary (default: starship)",
    )
    parser.add_argument(
        "--git-bin",
        default="git",
        help="Path to the git binary (default: git)",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=DEFAULT_ITERATIONS,
        help=f"Samples per workload (default: {DEFAULT_ITERATIONS})",
    )
    parser.add_argument("--json-out", type=Path, help="Path to write JSON report")
    parser.add_argument(
        "--markdown-out", type=Path, help="Path to write Markdown report"
    )
    return parser


def collect_metadata(
    *, iterations: int, capsule_bin: str, starship_bin: str, git_bin: str
) -> RunMetadata:
    def try_cmd(cmd: list[str]) -> str:
        try:
            return run_command(cmd)
        except (OSError, subprocess.CalledProcessError):
            return "unknown"

    return RunMetadata(
        iterations=iterations,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
        python=sys.version.split()[0],
        macos=try_cmd(["sw_vers", "-productVersion"]),
        kernel=try_cmd(["uname", "-srv"]),
        cpu=try_cmd(["sysctl", "-n", "machdep.cpu.brand_string"]),
    )


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    if args.iterations < 1:
        parser.error("--iterations must be at least 1")

    try:
        capsule_bin = resolve_binary(args.capsule_bin, "capsule")
        starship_bin = resolve_binary(args.starship_bin, "starship")
        git_bin = resolve_binary(args.git_bin, "git")
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="prompt-bench-") as temp_root:
        root = Path(temp_root)
        home_dir = root / "home"
        home_dir.mkdir()
        (home_dir / ".capsule").mkdir()

        workloads = create_workloads(root, git_bin, args.iterations)

        write_bench_config(home_dir)
        print("Starting capsule daemon...", file=sys.stderr, flush=True)
        try:
            daemon_proc = start_daemon(capsule_bin, home_dir)
        except RuntimeError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1

        try:
            results = benchmark(
                workloads=workloads,
                capsule_bin=capsule_bin,
                starship_bin=starship_bin,
                git_bin=git_bin,
                iterations=args.iterations,
                home_dir=home_dir,
            )
        except RuntimeError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        finally:
            stop_daemon(daemon_proc)

    metadata = collect_metadata(
        iterations=args.iterations,
        capsule_bin=capsule_bin,
        starship_bin=starship_bin,
        git_bin=git_bin,
    )

    markdown = render_markdown(metadata, results)
    print(markdown, end="")

    if args.markdown_out is not None:
        args.markdown_out.write_text(markdown, encoding="utf-8")

    if args.json_out is not None:
        payload = {
            "metadata": asdict(metadata),
            "results": [asdict(r) for r in results],
        }
        args.json_out.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
