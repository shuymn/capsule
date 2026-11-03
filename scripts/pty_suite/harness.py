"""Process/PTY control and the scenario-facing `Shell` DSL.

Scenarios drive zsh through `Shell` methods named for what they do
(`type_text`, `enter_vicmd`, `inject_frame`, `rendered`, ...). Control codes,
probe files, and log formats stay in this module and `fixtures`.
"""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import errno
import json
import os
from pathlib import Path
import re
import select
import shlex
import signal
import subprocess
import sys
import termios
import time

from . import contracts, fixtures, terminal

# Key bindings installed by fixtures.INSTRUMENTATION and vi-mode zsh.
KEY_STATE = b"\x0f"  # ^O: dump ZLE state to probe files
KEY_INJECT = b"\x10"  # ^P: feed $HOME/inject to _capsule_frame
KEY_CLEAR = b"\x15"  # ^U: clear the buffer and select viins
KEY_ESCAPE = b"\x1b"
KEY_LEFT = b"\x1b[D"

# The prompt Capsule assigns while PROMPT_SUBST expands the rendered text.
SUBST_PROMPT = b"${_CAPSULE_RENDERED}"
FALLBACK_PROMPT = b"%~\n%# "

STATE_FIELDS = ("cursor", "generation", "worker", "write_fd", "read_fd", "tx", "pending", "cols")


def active(pid: int) -> bool:
    result = subprocess.run(
        ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, timeout=3
    )
    state = result.stdout.strip()
    return bool(state) and not state.startswith(b"Z")


def worker_process(pid: int):
    """Return a live process's parent and group/start-time/command identity."""
    result = subprocess.run(
        ["ps", "-ww", "-o", "stat=", "-o", "ppid=", "-o", "pgid=",
         "-o", "lstart=", "-o", "command=", "-p", str(pid)],
        capture_output=True, timeout=3,
    )
    fields = result.stdout.split(None, 8)
    if not fields and not result.stderr.strip():
        return None
    if len(fields) != 9 or result.stderr.strip():
        raise RuntimeError(f"cannot inspect process {pid}: {result.stdout!r} {result.stderr!r}")
    if fields[0].startswith(b"Z"):
        return None
    # Ignore parent changes in the identity so normal EOF cleanup can be
    # observed after exit; signaling separately requires the original parent.
    identity = (int(fields[2]), tuple(fields[3:8]), fields[8].strip())
    return int(fields[1]), identity


def eventually(predicate, description: str, pump=lambda: None, timeout=8):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError(f"deadline waiting for {description}")
        pump()
        time.sleep(0.005)


def unescape(field: bytes) -> bytes:
    escapes = {b"\\": b"\\", b"t": b"\t", b"n": b"\n", b"r": b"\r"}
    return re.sub(rb"\\([\\tnr])", lambda match: escapes[match[1]], field)


def _bytes(value: str | bytes) -> bytes:
    return value.encode() if isinstance(value, str) else value


def frame_record(generation: int, line1: str | bytes, line2: str | bytes, complete: bool = True) -> bytes:
    """Encode a worker render frame in the form `_capsule_frame` receives."""
    return b"\t".join((b"R", str(generation).encode(), _bytes(line1), _bytes(line2), b"1" if complete else b"0"))


def parse_state(text: str) -> dict:
    fields = text.split()
    result = dict(zip(STATE_FIELDS, map(int, fields[:len(STATE_FIELDS)])))
    result["keymap"] = fields[len(STATE_FIELDS)]
    return result


def parse_transition(line: bytes) -> dict:
    event, gen, prompt, rendered, status, duration, cols, keymap, complete = line.split(b"\t")
    prompt, rendered = unescape(prompt), unescape(rendered)
    return {
        "event": event, "generation": int(gen), "prompt": prompt, "rendered": rendered,
        "effective": rendered if prompt == SUBST_PROMPT else prompt,
        "status": int(status), "duration": duration,
        "cols": int(cols), "keymap": keymap, "complete": complete,
    }


@dataclass(frozen=True)
class Prompt:
    """A submitted input line: the generation it starts and the log mark before it."""

    shell: Shell
    generation: int
    mark: tuple[int, int]

    def complete(self, timeout=8):
        self.shell.complete(self.generation, self.mark, timeout)

    def transitions(self):
        return self.shell.transitions(self.mark, self.generation)

    def finalized(self):
        return self.shell.finalized(self.mark, self.generation)

    def last_finalized(self) -> dict:
        finalized = self.finalized()
        assert finalized, ("no precmd finalization", self.generation)
        return finalized[-1]

    def renders(self):
        return self.shell.renders(self.mark, self.generation)


class Shell:
    def __init__(self, binary: Path, root: Path, name: str, hold=False, flicker=False, precmd_setup=""):
        self.home = root / name
        self.home.mkdir()
        (self.home / "work").mkdir()
        (self.home / ".capsule").mkdir()
        self.workers: set[int] = set()  # Historical diagnostics, never signal targets.
        self.live_workers = {}
        self.closed = False
        self.transcript = (self.home / "terminal.log").open("wb")
        script = self.home / "acquire.sh"
        script.write_text(fixtures.ACQUIRE_SCRIPT)
        self.write_config(fixtures.flicker_fixture(self.home) if flicker else fixtures.acquire_config(script))
        capsule_bin = self.home / "capsule-bin"
        capsule_bin.mkdir()
        (capsule_bin / "capsule").symlink_to(binary)
        path = f"{binary.parent}:/usr/bin:/bin"
        if flicker:
            path = f"{self.home / 'bin'}:{path}"
        # Generated integration uses `command capsule` for worker/fd-config.
        path = f"{capsule_bin}:{path}"
        init = subprocess.run([str(binary), "init", "zsh"], capture_output=True, check=True)
        self.write("init.zsh", init.stdout)
        self.write(".zshrc", fixtures.zshrc(path, precmd_setup))
        env = {
            "HOME": str(self.home), "ZDOTDIR": str(self.home),
            "PATH": path, "TERM": "xterm-256color",
            "LC_ALL": terminal.utf8_locale(),
            "CAP_REMOVE": "worker-start-value",
        }
        if hold:
            env["CAP_HOLD"] = "1"
        self.pid, self.fd = terminal.spawn_zsh(self.home / "work", env)
        self.resize(100)

    # Disposable-home files written by fixtures and probes.

    def path(self, name: str) -> Path:
        return self.home / name

    def read(self, name: str) -> bytes:
        return (self.home / name).read_bytes()

    def write(self, name: str, data: str | bytes) -> None:
        (self.home / name).write_bytes(_bytes(data))

    def exists(self, name: str) -> bool:
        return (self.home / name).exists()

    @property
    def config_path(self) -> Path:
        return self.home / ".capsule/config.toml"

    def write_config(self, text: str) -> None:
        self.config_path.write_text(text)

    # Values captured by the most recent `state()` probe.

    @property
    def rendered(self) -> bytes:
        return self.read("rendered")

    @property
    def prompt(self) -> bytes:
        return self.read("prompt")

    @property
    def buffer(self) -> bytes:
        return self.read("buffer")

    @property
    def effective_prompt(self) -> bytes:
        """What zsh displays: the rendered text when PROMPT expands it, else PROMPT."""
        prompt = self.prompt
        return self.rendered if prompt == SUBST_PROMPT else prompt

    # Values recorded by the most recent acquisition fixture run.

    def acquired(self, name: str) -> bytes:
        return self.read(name)

    @property
    def acquired_cwd(self) -> bytes:
        cwd = self.read("cwd")
        assert cwd.endswith(b"\n"), cwd
        return cwd[:-1]

    def count(self):
        """Number of acquisition fixture runs."""
        return len(self.read("count")) if self.exists("count") else 0

    # Terminal I/O.

    def pump(self):
        while select.select([self.fd], [], [], 0)[0]:
            try:
                chunk = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    return
                raise
            if not chunk:
                return
            self.transcript.write(chunk)
            self.transcript.flush()

    def output(self, since: int = 0) -> bytes:
        """Terminal output pumped so far, from byte offset `since`."""
        return self.read("terminal.log")[since:]

    def output_offset(self) -> int:
        self.pump()
        return self.path("terminal.log").stat().st_size

    def send(self, data: bytes):
        os.write(self.fd, data)

    def resize(self, cols: int):
        terminal.set_window_size(self.fd, 32, cols)
        os.kill(self.pid, signal.SIGWINCH)

    # ZLE operations.

    def type_text(self, text: str | bytes, move_left: int = 0) -> None:
        """Type into the ZLE buffer without submitting, then move the cursor left."""
        self.send(_bytes(text) + KEY_LEFT * move_left)

    def clear_line(self) -> None:
        """Clear the ZLE buffer and select viins (^U)."""
        self.send(KEY_CLEAR)

    def enter_vicmd(self) -> None:
        self.send(KEY_ESCAPE)

    def enter_viins(self) -> None:
        self.send(b"i")

    def inject_frame(self, generation: int, line1: str | bytes, line2: str | bytes, complete: bool = True) -> None:
        """Feed a synthetic worker frame through the ^P test widget."""
        self.write("inject", frame_record(generation, line1, line2, complete))
        self.send(KEY_INJECT)

    def command(self, source: str):
        self.path("zle-ready").unlink(missing_ok=True)
        self.send(source.encode() + b"\n")

    def press_enter(self, times: int = 1):
        self.path("zle-ready").unlink(missing_ok=True)
        self.send(b"\n" * times)

    def submit(self, source: str, generation=None, wait_ready=True) -> Prompt:
        """Submit one input line without waiting; `generation` defaults to the next one."""
        if generation is None:
            generation = self.next_generation(wait_ready)
        prompt = Prompt(self, generation, self.mark())
        self.command(source)
        return prompt

    def run_line(self, source, wait_ready=True) -> Prompt:
        """Submit one input line and wait until the generation it starts settles."""
        prompt = self.submit(source, wait_ready=wait_ready)
        prompt.complete()
        return prompt

    # Probes and waits.

    def ready(self, generation=None):
        target = self.path("zle-ready")
        eventually(
            lambda: target.exists() and (generation is None or target.read_text().strip() == str(generation)),
            "ZLE line init", self.pump,
        )

    def state(self, wait_ready=True):
        # A genuine shell error skips zsh's initial ZLE hook callbacks.
        if wait_ready:
            self.ready()
        else:
            eventually(
                lambda: not (termios.tcgetattr(self.fd)[3] & termios.ICANON),
                "ZLE raw input mode after hook error", self.pump,
            )
        target = self.path("state")
        target.unlink(missing_ok=True)
        self.send(KEY_STATE)
        eventually(target.exists, "ZLE state widget", self.pump)
        result = parse_state(target.read_text())
        if result["worker"]:
            pid = result["worker"]
            self.workers.add(pid)
            if pid not in self.live_workers:
                process = worker_process(pid)
                if process is not None:
                    # The worker makes itself a process-group leader before serving.
                    if process[0] != self.pid or process[1][0] != pid:
                        raise AssertionError(f"worker {pid} is not owned by shell {self.pid}")
                    self.live_workers[pid] = process[1]
                elif active(pid):
                    raise AssertionError(f"cannot identify active worker {pid}")
        return result

    def complete(self, generation: int, mark=(0, 0), timeout=8):
        def found():
            # Only apply records witness acceptance, including unchanged prompts.
            # The raw frame log also contains rejected stale/future responses.
            return any(
                item["event"] == b"apply" and item["complete"] == b"1"
                for item in self.transitions(mark, generation)
            )
        eventually(found, f"generation {generation} complete", self.pump, timeout=timeout)

    def next_generation(self, wait_ready=True):
        # Every prompt that reaches Capsule finalization starts a generation.
        return self.state(wait_ready)["generation"] + 1

    # Expectations.

    def assert_buffer(self, expected: bytes, cursor_offset: int = 0, wait_ready: bool = True) -> dict:
        """Probe ZLE and require the exact buffer and cursor (`len(expected) + cursor_offset`)."""
        state = self.state(wait_ready=wait_ready)
        actual = self.buffer
        assert actual == expected, f"buffer mismatch: {actual!r} != {expected!r}"
        expected_cursor = len(expected) + cursor_offset
        assert state["cursor"] == expected_cursor, f"cursor mismatch: {state['cursor']} != {expected_cursor}"
        return state

    def assert_no_injection(self, *marker_names: str) -> None:
        """Require that prompt expansion executed no command substitution."""
        for name in marker_names:
            assert not self.exists(name), f"injection marker {name!r} was created"

    def assert_acquired_cwd(self, directory: Path) -> None:
        assert self.acquired_cwd == os.fsencode(directory.resolve()), (self.acquired_cwd, directory)

    # Transition and frame logs.

    def log_lines(self, name):
        data = self.read(name) if self.exists(name) else b""
        # A callback may currently be appending its final record.
        return data.split(b"\n")[:-1]

    def mark(self):
        return len(self.log_lines("transitions")), len(self.log_lines("frames"))

    def transitions(self, mark, generation):
        return [
            item for item in map(parse_transition, self.log_lines("transitions")[mark[0]:])
            if item["generation"] == generation
        ]

    def finalized(self, mark, generation):
        """Transitions recorded when precmd finalization submitted the generation."""
        return [item for item in self.transitions(mark, generation) if item["event"] == b"precmd"]

    def frames(self, generation, mark=(0, 0)):
        """Raw (still escaped) `line1, line2, complete` fields of received frames."""
        prefix = f"R\t{generation}\t".encode()
        return [line.split(b"\t")[2:] for line in self.log_lines("frames")[mark[1]:] if line.startswith(prefix)]

    def renders(self, mark, generation):
        return [
            (unescape(line1) + b"\n" + unescape(line2) + b" ", complete == b"1")
            for line1, line2, complete in self.frames(generation, mark)
        ]

    # Lifecycle.

    def close(self, request_exit=True):
        if self.closed:
            return
        self.closed = True
        if request_exit:
            try:
                # Failed input-preservation probes may leave a partial command.
                self.send(KEY_CLEAR + b"exit\n")
            except OSError:
                pass
        def exited():
            try:
                return bool(os.waitpid(self.pid, os.WNOHANG)[0])
            except ChildProcessError:
                return True
        try:
            eventually(exited, "shell exit", self.pump, timeout=3)
        except AssertionError:
            os.kill(self.pid, signal.SIGKILL)
            eventually(exited, "forced shell exit", self.pump, timeout=3)
            raise
        finally:
            os.close(self.fd)
            self.transcript.close()

    def worker_gone(self, pid):
        identity = self.live_workers.get(pid)
        process = worker_process(pid)
        if identity is None or process is None or process[1] != identity:
            self.live_workers.pop(pid, None)
            return True
        return False

    def signal_worker(self, pid, sig):
        identity = self.live_workers.get(pid)
        process = worker_process(pid)
        if identity is None or process is None or process[1] != identity:
            self.live_workers.pop(pid, None)
            if sig == signal.SIGKILL:
                return
            raise AssertionError(f"worker {pid} has already exited")
        if process[0] != self.pid or self.closed:
            raise AssertionError(f"worker {pid} is no longer an identified child of shell {self.pid}")
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            self.live_workers.pop(pid, None)
            if sig != signal.SIGKILL:
                raise
            return
        if sig == signal.SIGKILL:
            eventually(lambda: self.worker_gone(pid), f"terminated worker {pid}")

    def cleanup_workers(self):
        # Resume a worker stopped by a failed backpressure probe, then let shell
        # EOF clean up its acquisitions. Do not SIGKILL a worker with descendants.
        # After shell exit, never signal remembered or reparented processes.
        if self.closed:
            return
        for pid in list(self.live_workers):
            if self.worker_gone(pid):
                continue
            process = worker_process(pid)
            if process is not None and process[0] == self.pid:
                self.signal_worker(pid, signal.SIGCONT)

    def assert_workers_gone(self):
        for pid in list(self.live_workers):
            eventually(lambda: self.worker_gone(pid), f"worker {pid} cleanup")


@contextmanager
def live_replacement(shell):
    """Hold a replacement alive while checking the exec/CLOEXEC boundary."""
    gate = shell.path("replacement-gate")
    os.mkfifo(gate)
    writer = os.open(gate, os.O_RDWR | os.O_NONBLOCK)
    fixture = shell.path("replacement.py")
    ready = shell.path("replacement-ready")
    try:
        fixture.write_text(fixtures.REPLACEMENT_SCRIPT)
        shell.command(f"exec {shlex.quote(sys.executable)} {shlex.quote(str(fixture))}")
        eventually(
            lambda: ready.exists() and ready.read_text() == str(shell.pid),
            "exec replacement ready", shell.pump,
        )
        assert active(shell.pid), "exec replacement exited before cleanup checks"
        yield
        assert active(shell.pid), "exec replacement exited during cleanup checks"
    finally:
        try:
            os.write(writer, b"release\n")
        finally:
            os.close(writer)
            shell.close(request_exit=False)


class Gates:
    """FIFO gates holding the flicker fixture's Git and tool acquisitions pending."""

    def __init__(self, shell):
        self.shell = shell
        self.generation = 1
        self.held = set()
        self.timings = []
        self.fds = {}
        for name in ("git", "tool"):
            fifo = shell.path(f"{name}.gate")
            os.mkfifo(fifo)
            self.fds[name] = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

    @contextmanager
    def stage(self, command):
        self.generation += 1
        mark = self.shell.mark()
        self.started = time.monotonic()
        for name in self.fds:
            for suffix in ("ready", "done"):
                self.shell.path(f"{name}.{suffix}").unlink(missing_ok=True)
            self.shell.path(f"{name}.armed").touch()
            self.held.add(name)
        self.shell.command(command)
        try:
            eventually(
                lambda: all(self.shell.exists(f"{name}.ready") for name in self.fds),
                "both gated acquisitions", self.shell.pump, timeout=0.35,
            )
            eventually(
                lambda: any(not done for _, done in self.shell.renders(mark, self.generation)),
                "pending render", self.shell.pump, timeout=0.1,
            )
            self.shell.ready(self.generation)
            yield mark, self.generation
        finally:
            self.release()

    def release(self, name=None):
        for current in list(self.held) if name is None else [name]:
            if current not in self.held:
                continue
            os.write(self.fds[current], b"release\n")
            self.shell.path(f"{current}.armed").unlink(missing_ok=True)
            self.held.remove(current)
            elapsed = time.monotonic() - self.started
            self.timings.append({"generation": self.generation, "fixture": current, "release_seconds": elapsed})
            assert elapsed < 0.45, f"gate held too close to the 500ms acquisition deadline: {elapsed:.3f}s"

    def wait_done(self, name, description):
        eventually(lambda: self.shell.exists(f"{name}.done"), description, self.shell.pump, timeout=0.1)

    def close(self):
        for fd in self.fds.values():
            os.close(fd)
        self.shell.write("gates.json", json.dumps(self.timings, indent=2) + "\n")


class Suite:
    """Owns the shells and result records of one driver run."""

    def __init__(self, binary: Path, output: Path):
        self.binary = binary
        self.output = output
        self.results = []
        self.shells = []

    def record(self, name, **details):
        if name not in contracts.RECORDS:
            raise AssertionError(f"record {name!r} is not listed in pty_suite/contracts.py")
        if any(result["test"] == name for result in self.results):
            raise AssertionError(f"record {name!r} was emitted twice")
        result = {"test": name, "passed": True, **details}
        self.results.append(result)
        print(json.dumps(result), flush=True)

    def start(self, name, hold=False, flicker=False, selected_binary=None, precmd_setup=""):
        shell = Shell(selected_binary or self.binary, self.output, name, hold, flicker, precmd_setup)
        self.shells.append(shell)
        return shell

    @contextmanager
    def session(self, name, **options):
        """Start a shell, then require clean exit and worker cleanup after the body.

        On failure, `finish` resumes stopped workers before closing the shell.
        """
        shell = self.start(name, **options)
        yield shell
        shell.close()
        shell.assert_workers_gone()

    def require_all_records(self):
        """Fail unless every registered contract record was emitted."""
        missing = contracts.missing_records(result["test"] for result in self.results)
        if missing:
            raise AssertionError("contract records missing: " + ", ".join(missing))

    def finish(self):
        cleanup_errors = []
        for shell in self.shells:
            for cleanup in (shell.cleanup_workers, shell.close, shell.assert_workers_gone):
                try:
                    cleanup()
                except Exception as error:
                    cleanup_errors.append(f"{shell.home.name}: {error}")
        (self.output / "results.json").write_text(json.dumps(self.results, indent=2) + "\n")
        if cleanup_errors:
            raise AssertionError("cleanup failed: " + "; ".join(cleanup_errors))
