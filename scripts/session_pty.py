#!/usr/bin/env python3
"""Exercise the real zsh/worker boundary in disposable homes (no benchmarks).

Run after `task build`: uv run scripts/session_pty.py --output /tmp/capsule-pty
The standard-library-only driver also runs with python3 on Linux CI images.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager, nullcontext
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


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


class Shell:
    def __init__(self, binary: Path, root: Path, name: str, hold=False, flicker=False):
        self.home = root / name
        self.home.mkdir()
        (self.home / "work").mkdir()
        (self.home / ".capsule").mkdir()
        self.workers: set[int] = set()  # Historical diagnostics, never signal targets.
        self.live_workers = {}
        self.closed = False
        self.transcript = (self.home / "terminal.log").open("wb")
        fixture = self.home / "acquire.sh"
        fixture.write_text(
            "#!/bin/sh\n"
            'printf x >> "$HOME/count"\n'
            'printf "%s" "${CAP_VALUE-unset}" > "$HOME/value"\n'
            'printf "%s" "${CAP_REMOVE-unset}" > "$HOME/removed"\n'
            'printf "%s" "${CAP_PRIVATE-unset}" > "$HOME/private"\n'
            'printf "%s" "${CAP_RAW-unset}" > "$HOME/raw"\n'
            'pwd > "$HOME/cwd"\n'
            'if [ "${CAP_GATE-}" = 1 ]; then\n'
            '  printf ready > "$HOME/gate-ready"\n'
            '  read -r ignored < "$HOME/gate"\n'
            'fi\n'
            'if [ "${CAP_HOLD-}" = 1 ]; then\n'
            '  sleep 60 & descendant=$!\n'
            '  printf "%s %s" "$$" "$descendant" > "$HOME/children"\n'
            '  wait "$descendant"\n'
            'fi\n'
            'printf "VALUE=%s" "${CAP_VALUE-initial}"\n'
        )
        config = (
            "schema_version = 2\n[git]\ndisabled = true\n"
            "[[module]]\nname = 'fixture'\nformat = '{value}'\n"
            f"[module.values]\nvalue = [{{command = ['/bin/sh', {json.dumps(str(fixture))}]}}]\n"
        )
        if flicker:
            config = flicker_fixture(self.home)
        (self.home / ".capsule/config.toml").write_text(config)
        capsule_bin = self.home / "capsule-bin"
        capsule_bin.mkdir()
        (capsule_bin / "capsule").symlink_to(binary)
        path = f"{binary.parent}:/usr/bin:/bin"
        if flicker:
            path = f"{self.home / 'bin'}:{path}"
        # Generated integration uses `command capsule` for worker/fd-config.
        path = f"{capsule_bin}:{path}"
        init = subprocess.run([str(binary), "init", "zsh"], capture_output=True, check=True)
        (self.home / "init.zsh").write_bytes(init.stdout)
        (self.home / ".zshrc").write_text(
            f"export PATH={shlex.quote(path)}\n"
            "unsetopt beep checkjobs\n"
            "setopt promptsubst\n"
            "bindkey -v\n"
            "KEYTIMEOUT=1\n"
            'source "$HOME/init.zsh"\n'
            "_capsule_test_transition() {\n"
            "  local field\n"
            "  {\n"
            '    print -rn -- "$1"\n'
            '    for field in "$_CAPSULE_GENERATION" "$PROMPT" "$_CAPSULE_RENDERED" "$_CAPSULE_LAST_EXIT" "$_CAPSULE_DURATION_MS" "${COLUMNS:-80}" "${KEYMAP:-main}" "${_capsule_test_complete:-}"; do\n'
            r"      field=${field//\\/\\\\}" "\n"
            r"      field=${field//$'\t'/\\t}" "\n"
            r"      field=${field//$'\n'/\\n}" "\n"
            r"      field=${field//$'\r'/\\r}" "\n"
            "      print -rn -- $'\\t'\"$field\"\n"
            "    done\n"
            "    print\n"
            '  } >> "$HOME/transitions"\n'
            "}\n"
            "functions[_capsule_test_original_apply]=$functions[_capsule_apply_prompt]\n"
            "_capsule_apply_prompt() {\n"
            '  _capsule_test_original_apply "$@"\n'
            "  _capsule_test_transition apply\n"
            "}\n"
            "functions[_capsule_test_original_precmd]=$functions[_capsule_precmd]\n"
            "_capsule_precmd() {\n"
            '  _capsule_test_original_precmd "$@"\n'
            "  _capsule_test_transition precmd\n"
            "}\n"
            "functions[_capsule_test_original_callback]=$functions[_capsule_async_callback]\n"
            "_capsule_async_callback() {\n"
            '  _capsule_test_original_callback "$@"\n'
            "  _capsule_test_transition callback\n"
            "}\n"
            "functions[_capsule_test_original_frame]=$functions[_capsule_frame]\n"
            "_capsule_frame() {\n"
            "  local _capsule_test_complete=${1##*$'\\t'}\n"
            '  _capsule_test_original_frame "$@"\n'
            '  print -r -- "$1" >> "$HOME/frames"\n'
            "}\n"
            "_capsule_test_state() {\n"
            '  print -rn -- "$BUFFER" > "$HOME/buffer"\n'
            '  print -rn -- "$PROMPT" > "$HOME/prompt"\n'
            '  print -rn -- "$_CAPSULE_RENDERED" > "$HOME/rendered"\n'
            '  print -r -- "$CURSOR $_CAPSULE_GENERATION $_CAPSULE_COPROC_PID $_CAPSULE_FD_IN $_CAPSULE_FD_OUT ${#_CAPSULE_TX} ${#_CAPSULE_PENDING} $_CAPSULE_LAST_COLS $KEYMAP" > "$HOME/state.tmp"\n'
            '  command /bin/mv "$HOME/state.tmp" "$HOME/state"\n'
            "}\n"
            "_capsule_test_inject() {\n"
            '  _CAPSULE_CHANGED=0\n'
            '  _capsule_frame "$(< "$HOME/inject")"\n'
            '  (( _CAPSULE_CHANGED )) && zle reset-prompt\n'
            "}\n"
            "_capsule_test_line_init() {\n"
            "  _capsule_test_transition line-init\n"
            '  print -r -- "$_CAPSULE_GENERATION" > "$HOME/zle-ready"\n'
            "}\n"
            "add-zle-hook-widget line-init _capsule_test_line_init\n"
            "_capsule_test_clear() { BUFFER=''; CURSOR=0; zle -K viins; }\n"
            "zle -N _capsule_test_state\n"
            "zle -N _capsule_test_inject\n"
            "zle -N _capsule_test_clear\n"
            "bindkey -M viins '^U' _capsule_test_clear\n"
            "bindkey -M vicmd '^U' _capsule_test_clear\n"
            "bindkey -M viins '^O' _capsule_test_state\n"
            "bindkey -M vicmd '^O' _capsule_test_state\n"
            "bindkey -M viins '^P' _capsule_test_inject\n"
            "bindkey -M vicmd '^P' _capsule_test_inject\n"
        )
        env = {
            "HOME": str(self.home), "ZDOTDIR": str(self.home),
            "PATH": path, "TERM": "xterm-256color",
            "LC_ALL": "en_US.UTF-8" if sys.platform == "darwin" else "C.UTF-8",
            "CAP_REMOVE": "worker-start-value",
        }
        if hold:
            env["CAP_HOLD"] = "1"
        zsh = shutil.which("zsh")
        if zsh is None:
            raise RuntimeError("zsh is required")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(self.home / "work")
            os.execve(zsh, [zsh, "-d", "-i"], env)
        self.resize(100)

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

    def send(self, data: bytes):
        os.write(self.fd, data)

    def command(self, source: str):
        (self.home / "zle-ready").unlink(missing_ok=True)
        self.send(source.encode() + b"\n")

    def ready(self, generation=None):
        target = self.home / "zle-ready"
        eventually(
            lambda: target.exists() and (generation is None or target.read_text().strip() == str(generation)),
            "ZLE line init", self.pump,
        )

    def resize(self, cols: int):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 32, cols, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)

    def state(self):
        self.ready()
        target = self.home / "state"
        target.unlink(missing_ok=True)
        self.send(b"\x0f")
        eventually(target.exists, "ZLE state widget", self.pump)
        fields = target.read_text().split()
        result = dict(zip(("cursor", "generation", "worker", "write_fd", "read_fd", "tx", "pending", "cols"), map(int, fields[:8])))
        result["keymap"] = fields[8]
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

    def count(self):
        path = self.home / "count"
        return len(path.read_bytes()) if path.exists() else 0

    def log_lines(self, name):
        path = self.home / name
        data = path.read_bytes() if path.exists() else b""
        # A callback may currently be appending its final record.
        return data.split(b"\n")[:-1]

    def mark(self):
        return len(self.log_lines("transitions")), len(self.log_lines("frames"))

    def transitions(self, mark, generation):
        result = []
        for line in self.log_lines("transitions")[mark[0]:]:
            event, gen, prompt, rendered, status, duration, cols, keymap, complete = line.split(b"\t")
            if int(gen) == generation:
                prompt, rendered = unescape(prompt), unescape(rendered)
                result.append({
                    "event": event, "prompt": prompt, "rendered": rendered,
                    "effective": rendered if prompt == b"${_CAPSULE_RENDERED}" else prompt,
                    "status": int(status), "duration": duration,
                    "cols": int(cols), "keymap": keymap, "complete": complete,
                })
        return result

    def renders(self, mark, generation):
        result = []
        prefix = f"R\t{generation}\t".encode()
        for line in self.log_lines("frames")[mark[1]:]:
            if line.startswith(prefix):
                _, _, line1, line2, complete = line.split(b"\t")
                result.append((unescape(line1) + b"\n" + unescape(line2) + b" ", complete == b"1"))
        return result

    def close(self, request_exit=True):
        if self.closed:
            return
        self.closed = True
        if request_exit:
            try:
                # Failed input-preservation probes may leave a partial command.
                self.send(b"\x15exit\n")
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
    gate = shell.home / "replacement-gate"
    os.mkfifo(gate)
    writer = os.open(gate, os.O_RDWR | os.O_NONBLOCK)
    fixture = shell.home / "replacement.py"
    ready = shell.home / "replacement-ready"
    try:
        fixture.write_text(
            "import os\nfrom pathlib import Path\n"
            'home = Path(os.environ["HOME"])\n'
            '(home / "replacement-ready").write_text(str(os.getpid()))\n'
            'with (home / "replacement-gate").open() as gate:\n'
            '    assert gate.readline() == "release\\n"\n'
        )
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


def unescape(field: bytes) -> bytes:
    escapes = {b"\\": b"\\", b"t": b"\t", b"n": b"\n", b"r": b"\r"}
    return re.sub(rb"\\([\\tnr])", lambda match: escapes[match[1]], field)


def flicker_fixture(home: Path) -> str:
    """File-controlled gates keep ordinary commands' exported snapshots identical."""
    (home / "work/.git").mkdir()
    (home / "bin").mkdir()
    for name in ("git", "tool"):
        script = home / "bin" / name
        action = (
            'printf "# branch.head %s\\n" "$mode"\n'
            if name == "git" else 'printf "%s" "$mode"\n'
        )
        script.write_text(
            "#!/bin/sh\n"
            + ('printf x >> "$HOME/count"\n' if name == "tool" else "")
            + f'mode=$(/bin/cat "$HOME/{name}.mode")\n'
            + f'if [ -f "$HOME/{name}.armed" ]; then\n'
            + f'  printf ready > "$HOME/{name}.ready"\n'
            + f'  read -r ignored < "$HOME/{name}.gate"\n'
            + "fi\n"
            + 'if [ "$mode" = failed ]; then\n  result=7\nelse\n'
            + action
            + "result=$?\nfi\n"
            + f'printf done > "$HOME/{name}.done"\n'
            + 'exit "$result"\n'
        )
        script.chmod(0o755)
        (home / f"{name}.mode").write_text("PTY_BRANCH_OLD" if name == "git" else "TOOL_OLD")
    (home / "work/version").write_text("FILE_OLD")
    (home / "work/condition").write_text("present")
    # Prepare fresh executables before zsh's first precmd, outside acquisition
    # deadlines. Setup must not leave counts or generation-completion witnesses.
    for name, expected in (("git", b"# branch.head PTY_BRANCH_OLD\n"), ("tool", b"TOOL_OLD")):
        result = subprocess.run(
            [str(home / "bin" / name)],
            cwd=home / "work",
            env={"HOME": str(home), "PATH": f"{home / 'bin'}:/usr/bin:/bin"},
            input=b"", capture_output=True, check=True, timeout=8,
        )
        assert result.stdout == expected, (name, result.stdout, result.stderr)
        assert (home / f"{name}.done").read_bytes() == b"done", name
    assert (home / "count").read_bytes() == b"x", "fixture setup must run tool exactly once"
    for name in ("count", "git.done", "tool.done"):
        (home / name).unlink()
    return (
        "schema_version = 2\n"
        "[git]\nicon = ''\nconnector = ''\n"
        "[cmd_duration]\nthreshold_ms = 0\nconnector = 'DURATION'\n"
        "[character]\nglyph = 'INPUT'\n"
        "success_style = {fg = 'green', bold = false}\n"
        "error_style = {fg = 'red', bold = false}\n"
        "[character.vicmd]\nglyph = 'NORMAL'\nstyle = {fg = 'blue', bold = false}\n"
        "[[module]]\nname = 'tool'\nformat = '{value}'\n"
        "[module.values]\nvalue = [{command = ['tool']}]\n"
        "[[module]]\nname = 'file'\nformat = '{value}'\n"
        "[module.values]\nvalue = [{file = 'version'}]\n"
        "[[module]]\nname = 'conditional'\nwhen.files = ['condition']\nformat = 'COND_OLD'\nvalues = {}\n"
        "[[module]]\nname = 'environment'\nformat = 'ENV={value}'\n"
        "[module.values]\nvalue = [{env = 'CAP_FLICKER'}]\n"
    )


class Gates:
    def __init__(self, shell):
        self.shell = shell
        self.generation = 1
        self.held = set()
        self.timings = []
        self.fds = {}
        for name in ("git", "tool"):
            fifo = shell.home / f"{name}.gate"
            os.mkfifo(fifo)
            self.fds[name] = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

    @contextmanager
    def stage(self, command):
        self.generation += 1
        mark = self.shell.mark()
        self.started = time.monotonic()
        for name in self.fds:
            for suffix in ("ready", "done"):
                (self.shell.home / f"{name}.{suffix}").unlink(missing_ok=True)
            (self.shell.home / f"{name}.armed").touch()
            self.held.add(name)
        self.shell.command(command)
        try:
            eventually(
                lambda: all((self.shell.home / f"{name}.ready").exists() for name in self.fds),
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
            (self.shell.home / f"{current}.armed").unlink(missing_ok=True)
            self.held.remove(current)
            elapsed = time.monotonic() - self.started
            self.timings.append({"generation": self.generation, "fixture": current, "release_seconds": elapsed})
            assert elapsed < 0.45, f"gate held too close to the 500ms acquisition deadline: {elapsed:.3f}s"

    def close(self):
        for fd in self.fds.values():
            os.close(fd)
        (self.shell.home / "gates.json").write_text(json.dumps(self.timings, indent=2) + "\n")


EXTERNAL_MARKERS = {
    b"PTY_BRANCH_OLD", b"PTY_BRANCH_NEW", b"TOOL_OLD", b"TOOL_NEW",
    b"FILE_OLD", b"COND_OLD", b"ENV=exported", b"RELOADED-",
}


def external(prompt):
    return {marker for marker in EXTERNAL_MARKERS if marker in prompt}


def assert_history(shell, mark, generation, pending, settled=None):
    """Check every emitted render and actual prompt, including precmd fallback."""
    renders = shell.renders(mark, generation)
    transitions = shell.transitions(mark, generation)
    assert renders and any(item["event"] == b"precmd" for item in transitions)
    if settled is not None:
        assert any(item["event"] == b"apply" and item["complete"] == b"1" for item in transitions), ("missing accepted completion", generation, transitions)
    settled = pending if settled is None else settled
    for prompt, complete in renders:
        expected = settled if complete else pending
        assert external(prompt) == expected, ("render", generation, complete, expected, prompt)
    for transition in transitions:
        if transition["event"] == b"apply":
            expected = settled if transition["complete"] == b"1" else pending
            assert external(transition["effective"]) == expected, ("apply", generation, expected, transition)
        else:
            # Callbacks can finish a batch containing the settled frame.
            assert external(transition["effective"]) in (pending, settled), ("prompt transition", generation, pending, transition)


def wait_pending(shell, mark, generation, predicate):
    eventually(
        lambda: any(not complete and predicate(prompt) for prompt, complete in shell.renders(mark, generation)),
        "local state rendered while acquisition is pending", shell.pump, timeout=0.2,
    )


def run_flicker(shell, record):
    shell.complete(1)
    assert shell.count() == 1
    old = {b"PTY_BRANCH_OLD", b"TOOL_OLD", b"FILE_OLD", b"COND_OLD"}
    shell.state()
    assert external((shell.home / "rendered").read_bytes()) == old
    gates = Gates(shell)
    try:
        with gates.stage("true") as (mark, generation):
            assert_history(shell, mark, generation, old)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, old, old)
        record("ordinary_command_preserves_git_and_tools_in_every_render_and_precmd")

        # Change local duration deterministically without sleeping or exporting a gate.
        with gates.stage("_CAPSULE_CMD_START=$((EPOCHREALTIME-3)); false") as (mark, generation):
            wait_pending(shell, mark, generation, lambda prompt: b"DURATION" in prompt and b"3s" in prompt and b"\x1b[31m%}INPUT" in prompt)
            assert_history(shell, mark, generation, old)
            shell.send(b"echo cursor-marker\x1b[D\x1b[D\x1b[D")
            original = shell.state()
            typed = (shell.home / "buffer").read_bytes()
            assert original["cursor"] == len(typed) - 3
            wide = (shell.home / "rendered").read_bytes().split(b"\n")[0]
            redraw = shell.mark()
            shell.resize(42)
            narrow_state = shell.state()  # Trigger zsh's line-pre-redraw hook.
            wait_pending(shell, redraw, generation, lambda prompt: prompt.split(b"\n")[0] != wide)
            assert narrow_state["cursor"] == original["cursor"]
            assert (shell.home / "buffer").read_bytes() == typed
            shell.state()
            narrow = (shell.home / "rendered").read_bytes().split(b"\n")[0]
            assert len(re.sub(rb"%\{.*?%\}", b"", narrow).decode()) <= 42
            redraw = shell.mark()
            shell.send(b"\x1b")
            shell.state()
            wait_pending(shell, redraw, generation, lambda prompt: b"NORMAL" in prompt)
            restore = shell.mark()
            shell.send(b"i")
            shell.resize(100)
            shell.state()
            wait_pending(shell, restore, generation, lambda prompt: external(prompt) == old and b"\x1b[31m%}INPUT" in prompt)
            before = shell.state()
            assert external((shell.home / "rendered").read_bytes()) == old
            assert (shell.home / "buffer").read_bytes() == typed
            gates.release()
            shell.complete(generation, mark)
            after = shell.state()
            assert (shell.home / "buffer").read_bytes() == typed
            assert before["cursor"] == after["cursor"]
            assert shell.count() == generation
            # Narrow-width truncation is intentional; every other render keeps
            # the settled external snapshot and uses the new local status.
            for prompt, _ in shell.renders(mark, generation):
                assert b"\x1b[31m%}INPUT" in prompt or b"\x1b[34m%}NORMAL" in prompt, prompt
                observed = external(prompt)
                assert observed <= old, prompt
                if observed != old:
                    assert len(re.sub(rb"%\{.*?%\}", b"", prompt.split(b"\n")[0]).decode()) <= 42, prompt
                else:
                    assert b"DURATION" in prompt and b"3s" in prompt, prompt
            for transition in shell.transitions(mark, generation):
                if transition["event"] == b"apply":
                    assert transition["prompt"] == b"${_CAPSULE_RENDERED}", transition
            # Receipt-time COLUMNS is not the render width: an in-flight narrow
            # response may arrive after expansion. Require full markers only
            # once a restored wide insert-mode prompt has actually been applied.
            restored_history = shell.transitions(restore, generation)
            wide_restored = False
            for transition in restored_history:
                if (transition["event"] == b"apply"
                        and external(transition["effective"]) == old
                        and b"\x1b[31m%}INPUT" in transition["effective"]):
                    wide_restored = True
                if wide_restored:
                    assert external(transition["effective"]) == old, transition
            assert wide_restored, restored_history
        shell.send(b"\x15")
        record("pending_acquisition_renders_status_duration_resize_and_vi_without_losing_input")

        (shell.home / "git.mode").write_text("PTY_BRANCH_NEW")
        (shell.home / "tool.mode").write_text("TOOL_NEW")
        (shell.home / "work/version").unlink()
        (shell.home / "work/condition").unlink()
        changed = {b"PTY_BRANCH_NEW", b"TOOL_NEW"}
        with gates.stage("true") as (mark, generation):
            gates.release("git")
            eventually((shell.home / "git.done").exists, "Git finishes before tool", shell.pump, timeout=0.1)
            redraw = shell.mark()
            shell.resize(99)
            shell.state()
            wait_pending(shell, redraw, generation, lambda prompt: external(prompt) == old)
            assert_history(shell, mark, generation, old)
            # Old/future responses must not replace the retained pending display.
            for wrong in (generation - 1, generation + 1):
                (shell.home / "inject").write_text(f"R\t{wrong}\twrong-generation\twrong\t1")
                shell.send(b"\x10")
                shell.state()
            assert_history(shell, mark, generation, old)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, old, changed)
        shell.resize(100)
        record("external_snapshot_commits_atomically_and_clears_missing_values_and_false_conditions")

        (shell.home / "tool.mode").write_text("failed")
        with gates.stage("true") as (mark, generation):
            # The previous stage injected this generation's completed frame while
            # it was still in the future. It remains in the raw log, not apply.
            assert any(complete and b"wrong-generation" in prompt for prompt, complete in shell.renders((0, 0), generation))
            try:
                shell.complete(generation, timeout=0)
            except AssertionError as error:
                assert str(error) == f"deadline waiting for generation {generation} complete", error
            else:
                raise AssertionError("rejected future frame satisfied pending completion")
            gates.release("tool")
            eventually((shell.home / "tool.done").exists, "failed tool finishes before Git", shell.pump, timeout=0.1)
            redraw = shell.mark()
            shell.resize(99)
            shell.state()
            wait_pending(shell, redraw, generation, lambda prompt: external(prompt) == changed)
            assert_history(shell, mark, generation, changed)
        shell.complete(generation, mark)
        failed = {b"PTY_BRANCH_NEW"}
        assert_history(shell, mark, generation, changed, failed)
        record("rejected_future_completion_does_not_satisfy_pending_generation")
        record("failed_value_is_retained_pending_then_cleared_when_settled")

        (shell.home / "tool.mode").write_text("TOOL_NEW")
        (shell.home / "work/other").mkdir()
        with gates.stage("cd other") as (mark, generation):
            assert_history(shell, mark, generation, set())
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, set(), changed)
        record("changed_cwd_discards_settled_external_display_immediately")

        exported = changed | {b"ENV=exported"}
        with gates.stage("export CAP_FLICKER=exported") as (mark, generation):
            assert_history(shell, mark, generation, set())
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, set(), exported)
        record("changed_exported_environment_discards_settled_external_display_immediately")

        config_path = shell.home / ".capsule/config.toml"
        config = config_path.read_text().replace("name = 'tool'\nformat = '{value}'", "name = 'tool'\nformat = 'RELOADED-{value}'")
        config_path.write_text(config)
        reloaded = exported | {b"RELOADED-"}
        with gates.stage("true") as (mark, generation):
            # The precmd may preserve the old display until asynchronous Plan load.
            wait_pending(shell, mark, generation, lambda prompt: not external(prompt))
            transitions = shell.transitions(mark, generation)
            cleared = next(index for index, item in enumerate(transitions) if item["event"] == b"apply" and not external(item["effective"]))
            assert all(not external(item["effective"]) for item in transitions[cleared:]), transitions
            # Keep the after-load boundary for all remaining pending/settled frames.
            after_load = shell.mark()
            shell.resize(100)
            shell.state()
            wait_pending(shell, after_load, generation, lambda prompt: not external(prompt))
        shell.complete(generation, mark)
        # Validate every frame/application across Plan load, not only the final prompt.
        cleared = False
        for prompt, complete in shell.renders(mark, generation):
            observed = external(prompt)
            if complete:
                assert observed == reloaded, (complete, prompt)
            else:
                cleared = cleared or not observed
                assert observed == (set() if cleared else exported), prompt
        cleared = False
        for item in shell.transitions(mark, generation):
            observed = external(item["effective"])
            if item["event"] == b"apply":
                if item["complete"] == b"1":
                    assert observed == reloaded, item
                else:
                    cleared = cleared or not observed
                    assert observed == (set() if cleared else exported), item
            else:
                assert observed in (set(), exported, reloaded), item
        shell.state()
        assert external((shell.home / "rendered").read_bytes()) == reloaded
        record("changed_valid_config_clears_display_after_plan_load_before_acquisition_finishes")

        config_path.write_text("schema_version = [invalid\n")
        with gates.stage("true") as (mark, generation):
            assert_history(shell, mark, generation, reloaded)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, reloaded, reloaded)
        record("invalid_config_retains_previous_valid_plan_and_settled_display")
        config_path.write_text(config)
        record("slow_fixtures_release_before_acquisition_deadline", max_release_seconds=max(item["release_seconds"] for item in gates.timings))
    finally:
        gates.close()


def run(binary: Path, output: Path):
    results = []
    shells = []
    def record(name, **details):
        result = {"test": name, "passed": True, **details}
        results.append(result)
        print(json.dumps(result), flush=True)
    def start(name, hold=False, flicker=False, selected_binary=None):
        shell = Shell(selected_binary or binary, output, name, hold, flicker)
        shells.append(shell)
        return shell
    try:
        # A renamed selected executable must win over a conflicting `capsule`.
        binary_fixture = output / "binary-selection-bin"
        binary_fixture.mkdir()
        candidate = binary_fixture / "capsule-candidate"
        candidate.symlink_to(binary)
        conflicting = binary_fixture / "capsule"
        conflicting.write_text('#!/bin/sh\nprintf unexpected > "$HOME/wrong-binary"\nexit 97\n')
        conflicting.chmod(0o755)
        shell = start("binary-selection", selected_binary=candidate)
        shell.complete(1)
        assert shell.state()["worker"] and shell.count() == 1
        assert not (shell.home / "wrong-binary").exists()
        shell.close()
        shell.assert_workers_gone()
        record("renamed_selected_binary_provides_init_worker_and_fd_config")

        # Simulate reuse of an already-retired worker PID by an unrelated process.
        assert shell.workers and not shell.live_workers
        unrelated = subprocess.Popen(["/bin/sleep", "30"])
        try:
            shell.workers.add(unrelated.pid)
            shell.cleanup_workers()
            assert unrelated.poll() is None, "historical PID was signaled during cleanup"
        finally:
            shell.workers.discard(unrelated.pid)
            unrelated.terminate()
            unrelated.wait(timeout=3)
        record("retired_worker_pids_are_not_cleanup_targets")

        shell = start("functional")
        shell.complete(1)
        original = shell.state()
        assert shell.count() == 1
        initial_prompt = (shell.home / "rendered").read_bytes()
        initial_expansion = (shell.home / "prompt").read_bytes()
        assert b"VALUE=initial" in initial_prompt
        record("one_shell_starts_one_worker", worker=original["worker"])

        for _ in range(5):
            shell.send(b"\n")
            state = shell.state()
            assert state["generation"] == 1 and state["worker"] == original["worker"]
            assert (shell.home / "rendered").read_bytes() == initial_prompt
            assert (shell.home / "prompt").read_bytes() == initial_expansion
        assert shell.count() == 1
        record("empty_enter_preserves_information_and_reuses_acquisitions", enters=5)

        shell.resize(70)
        eventually(lambda: shell.state()["cols"] == 70, "resize request", shell.pump)
        shell.send(b"\x1b")
        eventually(lambda: shell.state()["keymap"] == "vicmd", "vi command mode", shell.pump)
        eventually(lambda: shell.state() and "❮".encode() in (shell.home / "rendered").read_bytes(), "vi glyph", shell.pump)
        shell.send(b"i")
        eventually(lambda: shell.state()["keymap"] in ("main", "viins"), "vi insert mode", shell.pump)
        assert shell.count() == 1
        record("resize_and_keymap_rerender_without_acquisition")

        shell.command("unset CAP_REMOVE; CAP_PRIVATE=private; export CAP_VALUE=$'日本語\\tline\\n\\\\tail'; export CAP_RAW=$'raw-\\xff'")
        shell.complete(2)
        assert (shell.home / "value").read_bytes() == "日本語\tline\n\\tail".encode()
        assert (shell.home / "raw").read_bytes() == b"raw-\xff"
        assert (shell.home / "removed").read_bytes() == b"unset"
        assert (shell.home / "private").read_bytes() == b"unset"
        record("full_export_snapshot_preserves_bytes_and_removes_absent_variables")

        raw_cwd = os.fsencode(shell.home / "work") + b"/raw-\xff"
        non_utf8 = True
        try:
            os.mkdir(raw_cwd)
            shell.command("cd $'raw-\\xff'")
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            # APFS can reject invalid UTF-8 names; Linux exercises that boundary.
            non_utf8 = False
            directory = shell.home / "work" / "日本語 cwd"
            directory.mkdir()
            raw_cwd = os.fsencode(directory)
            shell.command("cd '日本語 cwd'")
        shell.complete(3)
        assert (shell.home / "cwd").read_bytes() == raw_cwd + b"\n"
        record("cwd_bytes_reach_acquisition", non_utf8_fixture_supported=non_utf8)

        os.mkfifo(shell.home / "gate")
        shell.command("export CAP_GATE=1 CAP_VALUE=after-gate")
        eventually((shell.home / "gate-ready").exists, "gated acquisition", shell.pump)
        shell.ready(4)
        shell.send(b"echo cursor-marker\x1b[D\x1b[D\x1b[D")
        before = shell.state()
        typed = (shell.home / "buffer").read_bytes()
        gate = os.open(shell.home / "gate", os.O_WRONLY | os.O_NONBLOCK)
        os.write(gate, b"release\n")
        os.close(gate)
        shell.complete(4)
        after = shell.state()
        assert typed == b"echo cursor-marker"
        assert (shell.home / "buffer").read_bytes() == typed
        assert before["cursor"] == after["cursor"] == len(typed) - 3
        assert b"VALUE=after-gate" in (shell.home / "rendered").read_bytes()
        record("asynchronous_prompt_update_preserves_typed_buffer_and_cursor")
        shell.send(b"\x03")
        shell.command("unset CAP_GATE; export CAP_VALUE=bulk")
        shell.complete(5)

        state = shell.state()
        shell.signal_worker(state["worker"], signal.SIGSTOP)
        shell.command("export CAP_BULK_A=${(l:95000::x:)} CAP_BULK_B=${(l:95000::y:)}")
        pending = shell.state()
        assert pending["tx"] > 0, pending
        assert pending["generation"] == 6
        shell.signal_worker(state["worker"], signal.SIGCONT)
        # No keyboard input until the worker finishes: credit notifications must progress TX.
        shell.complete(6)
        drained = shell.state()
        assert drained["tx"] == drained["pending"] == 0
        record("backpressure_completes_large_request_without_more_keyboard_input", queued_bytes=pending["tx"])

        current = shell.state()
        prompt = (shell.home / "rendered").read_bytes()
        for generation in (current["generation"] - 1, current["generation"] + 1):
            (shell.home / "inject").write_text(f"R\t{generation}\twrong-generation\twrong\t1")
            shell.send(b"\x10")
            shell.state()
            assert (shell.home / "rendered").read_bytes() == prompt
        record("stale_and_future_responses_do_not_replace_prompt")

        shell.signal_worker(current["worker"], signal.SIGKILL)
        eventually(lambda: shell.state()["read_fd"] == 0, "worker failure fallback", shell.pump)
        assert (shell.home / "prompt").read_bytes() == b"%~\n%# "
        shell.command("unset CAP_BULK_A CAP_BULK_B")
        shell.complete(7)
        restarted = shell.state()
        assert restarted["worker"] != current["worker"]
        record("worker_failure_uses_fallback_and_next_command_restarts_worker")
        shell.close()
        shell.assert_workers_gone()

        for mode in ("exit", "exec"):
            shell = start("cleanup-" + mode, hold=True)
            eventually((shell.home / "children").exists, "active acquisition descendants", shell.pump)
            shell.state()
            children = list(map(int, (shell.home / "children").read_text().split()))
            if mode == "exit":
                shell.close()
            with (live_replacement(shell) if mode == "exec" else nullcontext()):
                shell.assert_workers_gone()
                for pid in children:
                    eventually(lambda pid=pid: not active(pid), "acquisition descendant cleanup")
            record(mode + "_closes_cloexec_pipes_and_cleans_active_process_group")

        many = [start(f"parallel-{index}") for index in range(10)]
        worker_ids = set()
        for shell in many:
            shell.complete(1)
            worker_ids.add(shell.state()["worker"])
            assert shell.count() == 1
        assert len(worker_ids) == 10 and all(active(pid) for pid in worker_ids)
        for shell in many:
            shell.close()
        for shell in many:
            shell.assert_workers_gone()
        record("ten_shells_own_ten_workers_and_leave_none_active")

        shell = start("flicker", flicker=True)
        run_flicker(shell, record)
        shell.close()
        shell.assert_workers_gone()
    finally:
        cleanup_errors = []
        for shell in shells:
            for cleanup in (shell.cleanup_workers, shell.close, shell.assert_workers_gone):
                try:
                    cleanup()
                except Exception as error:
                    cleanup_errors.append(f"{shell.home.name}: {error}")
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        if cleanup_errors:
            raise AssertionError("cleanup failed: " + "; ".join(cleanup_errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/capsule"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    output = args.output or Path(tempfile.mkdtemp(prefix="capsule-pty-"))
    output.mkdir(parents=True, exist_ok=True)
    print(f"PTY artifacts: {output}", flush=True)
    run(args.binary.resolve(), output.resolve())


if __name__ == "__main__":
    main()
