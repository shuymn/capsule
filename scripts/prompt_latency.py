#!/usr/bin/env python3
"""Compare user-visible prompt latency of Capsule, Starship, and a plain zsh prompt.

Run with `task bench:latency` or directly:
    uv run --no-project scripts/prompt_latency.py --capsule-bin target/release/capsule \
        --starship-bin "$(command -v starship)" --output <new-directory>

Each sample switches the repository's branch from outside the shell, presses Enter on an
empty line, and records two times from the keypress:
- ready: ZLE accepts input again (zle-line-init), i.e. how long typing is blocked
- fresh: the new branch name is visible in the prompt

Tools run in interleaved order on the same host, so results compare tools on that host
only. See docs/benchmarking.md for the measurement boundary and publication rules.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import platform
import pty
import random
import select
import shlex
import shutil
import statistics
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path

READY = b"\x1b]1337;BENCH-READY\x07"
TIMEOUT = 10.0


def git(repo: Path, *args: str, env: dict[str, str]) -> None:
    subprocess.run(["git", "-C", str(repo), *args], env=env, check=True, capture_output=True)


def create_repository(path: Path, files: int, env: dict[str, str]) -> None:
    path.mkdir(parents=True)
    git(path, "init", "-q", "-b", "main", env=env)
    for index in range(files):
        directory = path / f"d{index // 100:04d}"
        directory.mkdir(exist_ok=True)
        (directory / f"f{index % 100:02d}.txt").write_text(f"{index}\n")
    git(path, "add", "-A", env=env)
    git(path, "-c", "user.name=bench", "-c", "user.email=bench@example.invalid",
        "commit", "-q", "-m", "fixture", env=env)
    # Modified and untracked files make every status call do real comparison work.
    for index in range(0, min(files, 200)):
        (path / f"d{index // 100:04d}" / f"f{index % 100:02d}.txt").write_text("changed\n")
    for index in range(min(files, 200)):
        (path / f"untracked-{index:03d}.txt").write_text("new\n")


def host_fpath() -> str:
    # Read zsh's function path under the caller's HOME; the fixture HOME may hide it.
    return subprocess.run(["zsh", "-fc", "print -r -- ${(j.:.)fpath}"], capture_output=True,
                          text=True, check=True).stdout.strip()


def zshrc(tool: str, path: str, fpath: str, exports: int) -> str:
    init = {
        "capsule": 'eval "$(capsule init zsh)"\n',
        "starship": 'eval "$(starship init zsh)"\n',
        "plain": "PROMPT='%~ %# '\n",
    }[tool]
    # Interactive shells usually export many variables; every tool receives the same set.
    exported = "".join(f"export BENCH_VAR_{index:03d}=value-{index}\n" for index in range(exports))
    # Global zshenv files may rewrite PATH and fpath; restore the fixture values first.
    return (
        f"export PATH={shlex.quote(path)}\n"
        f"FPATH={shlex.quote(fpath)}\n"
        + exported
        + "unsetopt beep\n"
        + init
        # zle-hook registration fails silently until zsh/zle is loaded.
        + "zmodload zsh/zle\nautoload -Uz add-zle-hook-widget\n"
        + "_bench_ready() { print -rn -- $'\\e]1337;BENCH-READY\\a' }\n"
        + "add-zle-hook-widget line-init _bench_ready\n"
    )


class Session:
    def __init__(self, tool: str, home: Path, cwd: Path, path: str, fpath: str, exports: int):
        home.mkdir(parents=True)
        (home / ".zshrc").write_text(zshrc(tool, path, fpath, exports))
        self.tool = tool
        self.buffer = bytearray()
        self.stamps: list[tuple[int, float]] = []  # (buffer length, read time)
        env = {"HOME": str(home), "ZDOTDIR": str(home), "PATH": path, "TERM": "xterm-256color",
               "LC_ALL": "en_US.UTF-8" if sys.platform == "darwin" else "C.UTF-8",
               "GIT_CONFIG_NOSYSTEM": "1"}
        zsh = shutil.which("zsh")
        if zsh is None:
            raise RuntimeError("zsh is required")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            os.execve(zsh, [zsh, "-d", "-i"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))

    def pump(self, wait: float) -> None:
        if select.select([self.fd], [], [], wait)[0]:
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                return
            now = time.perf_counter()
            self.buffer += chunk
            self.stamps.append((len(self.buffer), now))

    def wait_for(self, needle: bytes, start: int, deadline: float) -> float:
        """Return the read time of the chunk that completed `needle` after `start`."""
        while True:
            index = self.buffer.find(needle, start)
            if index >= 0:
                end = index + len(needle)
                return next(stamp for length, stamp in self.stamps if length >= end)
            remaining = deadline - time.perf_counter()
            if remaining <= 0:
                raise TimeoutError(f"{self.tool}: {needle!r} not seen")
            self.pump(min(remaining, 0.05))

    def drain(self, quiet: float = 0.2) -> None:
        """Read until output stays quiet, so late redraws cannot leak into a sample."""
        last = time.perf_counter()
        while time.perf_counter() - last < quiet:
            before = len(self.buffer)
            self.pump(0.02)
            if len(self.buffer) != before:
                last = time.perf_counter()

    def sample(self, repo: Path, branch: str, env: dict[str, str]) -> dict[str, float]:
        git(repo, "symbolic-ref", "HEAD", f"refs/heads/{branch}", env=env)
        self.drain()
        start = len(self.buffer)
        sent = time.perf_counter()
        os.write(self.fd, b"\n")
        deadline = sent + TIMEOUT
        ready = self.wait_for(READY, start, deadline)
        result = {"ready_ms": (ready - sent) * 1000}
        if self.tool != "plain":
            fresh = self.wait_for(branch.encode(), start, deadline)
            result["fresh_ms"] = (fresh - sent) * 1000
        return result

    def close(self) -> None:
        try:
            os.write(self.fd, b"exit\n")
            deadline = time.perf_counter() + 3
            while time.perf_counter() < deadline:
                if os.waitpid(self.pid, os.WNOHANG)[0]:
                    break
                self.pump(0.05)
            else:
                os.kill(self.pid, 9)
                os.waitpid(self.pid, 0)
        except (OSError, ChildProcessError):
            pass
        os.close(self.fd)


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(fraction * len(ordered)))]


def command_output(argv: list[str]) -> str:
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return "unavailable"


def environment(args) -> dict[str, str]:
    if sys.platform == "darwin":
        cpu = command_output(["sysctl", "-n", "machdep.cpu.brand_string"])
    else:
        cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                    if line.startswith("model name")), platform.machine())
    return {
        "platform": f"{platform.system()} {platform.release()} {platform.machine()}",
        "cpu": cpu,
        "cpus": str(os.cpu_count()),
        "zsh": command_output(["zsh", "--version"]),
        "git": command_output(["git", "--version"]),
        "capsule": command_output([str(args.capsule_bin), "--version"]),
        "starship": command_output([str(args.starship_bin), "--version"]).splitlines()[0],
    }


def git_status_ms(repo: Path, env: dict[str, str], samples: int = 15) -> float:
    times = []
    for _ in range(samples):
        start = time.perf_counter()
        git(repo, "status", "--porcelain=v2", "--branch", env=env)
        times.append((time.perf_counter() - start) * 1000)
    return statistics.median(times)


def markdown(report) -> str:
    env = report["environment"]
    lines = [
        "## Prompt latency",
        "",
        f"{env['platform']}, {env['cpu']} ({env['cpus']} CPUs); {env['zsh']}; {env['git']}; "
        f"{env['capsule']}; {env['starship']}. {report['rounds']} interleaved samples per cell, "
        f"{report['exports']} extra exported variables, "
        "default configuration of each tool, empty Enter after an external branch switch.",
        "",
        "| Workload | `git status` p50 | Tool | Input blocked p50 / p90 | Fresh branch shown p50 / p90 |",
        "|---|---|---|---|---|",
    ]
    for workload in report["workloads"]:
        for tool, samples in workload["tools"].items():
            ready = [s["ready_ms"] for s in samples]
            fresh = [s["fresh_ms"] for s in samples if "fresh_ms" in s]
            fresh_cell = f"{percentile(fresh, .5):.1f} / {percentile(fresh, .9):.1f} ms" if fresh else "n/a"
            lines.append(
                f"| {workload['name']} ({workload['files']} files) | {workload['git_status_ms']:.1f} ms | {tool} "
                f"| {percentile(ready, .5):.1f} / {percentile(ready, .9):.1f} ms | {fresh_cell} |"
            )
    lines += ["", "Input blocked: Enter until zsh accepts input again. "
              "Fresh branch shown: Enter until the switched branch name is displayed."]
    return "\n".join(lines) + "\n"


def run(args) -> dict:
    root = args.output
    git_env = {"HOME": str(root / "git-home"), "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
               "GIT_CONFIG_NOSYSTEM": "1"}
    (root / "git-home").mkdir()
    bins = {}
    for tool, binary in (("capsule", args.capsule_bin), ("starship", args.starship_bin)):
        directory = root / f"{tool}-bin"
        directory.mkdir()
        (directory / tool).symlink_to(Path(binary).resolve())
        bins[tool] = directory
    git_dir = Path(shutil.which("git") or "/usr/bin/git").parent
    fpath = host_fpath()
    report = {"rounds": args.rounds, "exports": args.exports, "environment": environment(args), "workloads": []}
    rng = random.Random(args.seed)
    for name, files in (("small", 10), ("large", args.files)):
        repo = root / name / "repo"
        create_repository(repo, files, git_env)
        branches = [f"bench-{name}-{index:03d}" for index in range(args.rounds + args.warmup)]
        for branch in branches:
            git(repo, "branch", "-f", branch, "HEAD", env=git_env)
        sessions = {}
        for tool in ("capsule", "starship", "plain"):
            path = ":".join(str(p) for p in ([bins[tool]] if tool in bins else []) + [git_dir, Path("/usr/bin"), Path("/bin")])
            session = Session(tool, root / name / f"{tool}-home", repo, path, fpath, args.exports)
            session.wait_for(READY, 0, time.perf_counter() + TIMEOUT)
            sessions[tool] = session
        samples = {tool: [] for tool in sessions}
        try:
            for index, branch in enumerate(branches):
                order = list(sessions)
                rng.shuffle(order)
                # Every session still shows the previous branch, so each sample sees a real change.
                for tool in order:
                    result = sessions[tool].sample(repo, branch, git_env)
                    if index >= args.warmup:
                        samples[tool].append(result)
        finally:
            for session in sessions.values():
                session.close()
        report["workloads"].append({"name": name, "files": files,
                                    "git_status_ms": git_status_ms(repo, git_env), "tools": samples})
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--capsule-bin", type=Path, default=Path("target/release/capsule"))
    parser.add_argument("--starship-bin", type=Path, default=Path(shutil.which("starship") or "starship"))
    parser.add_argument("--rounds", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--files", type=int, default=50000)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--exports", type=int, default=100, help="extra exported variables per shell")
    parser.add_argument("--output", type=Path, required=True, help="new directory for fixtures and results")
    args = parser.parse_args()
    if args.rounds < 1 or args.warmup < 0 or args.files < 10:
        parser.error("--rounds must be >= 1, --warmup >= 0, --files >= 10")
    # Sessions change directory before zsh reads ZDOTDIR, so fixture paths must be absolute.
    args.output = args.output.absolute()
    args.output.mkdir(mode=0o700)
    report = run(args)
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    summary = markdown(report)
    (args.output / "results.md").write_text(summary)
    print(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main())
