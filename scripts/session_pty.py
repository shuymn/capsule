#!/usr/bin/env python3
"""Exercise the real zsh/worker boundary in disposable homes (no benchmarks).

Run after `task build`: uv run scripts/session_pty.py --output /tmp/capsule-pty
The standard-library-only driver also runs with python3 on Linux CI images.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
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


def eventually(predicate, description: str, pump=lambda: None, timeout=8):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError(f"deadline waiting for {description}")
        pump()
        time.sleep(0.005)


class Shell:
    def __init__(self, binary: Path, root: Path, name: str, hold=False):
        self.home = root / name
        self.home.mkdir()
        (self.home / "work").mkdir()
        (self.home / ".capsule").mkdir()
        self.workers: set[int] = set()
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
        (self.home / ".capsule/config.toml").write_text(config)
        init = subprocess.run([str(binary), "init", "zsh"], capture_output=True, check=True)
        (self.home / "init.zsh").write_bytes(init.stdout)
        (self.home / ".zshrc").write_text(
            f"export PATH={shlex.quote(str(binary.parent) + ':/usr/bin:/bin')}\n"
            "unsetopt beep checkjobs\n"
            "setopt promptsubst\n"
            "bindkey -v\n"
            'source "$HOME/init.zsh"\n'
            "functions[_capsule_test_original_frame]=$functions[_capsule_frame]\n"
            "_capsule_frame() {\n"
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
            "zle -N _capsule_test_state\n"
            "zle -N _capsule_test_inject\n"
            "bindkey -M viins '^O' _capsule_test_state\n"
            "bindkey -M vicmd '^O' _capsule_test_state\n"
            "bindkey -M viins '^P' _capsule_test_inject\n"
            "bindkey -M vicmd '^P' _capsule_test_inject\n"
        )
        env = {
            "HOME": str(self.home), "ZDOTDIR": str(self.home),
            "PATH": f"{binary.parent}:/usr/bin:/bin", "TERM": "xterm-256color",
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
        self.send(source.encode() + b"\n")

    def resize(self, cols: int):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 32, cols, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)

    def state(self):
        target = self.home / "state"
        target.unlink(missing_ok=True)
        self.send(b"\x0f")
        eventually(target.exists, "ZLE state widget", self.pump)
        fields = target.read_text().split()
        result = dict(zip(("cursor", "generation", "worker", "write_fd", "read_fd", "tx", "pending", "cols"), map(int, fields[:8])))
        result["keymap"] = fields[8]
        if result["worker"]:
            self.workers.add(result["worker"])
        return result

    def complete(self, generation: int):
        prefix = f"R\t{generation}\t".encode()
        def found():
            path = self.home / "frames"
            return path.exists() and any(line.startswith(prefix) and line.endswith(b"\t1") for line in path.read_bytes().splitlines())
        eventually(found, f"generation {generation} complete", self.pump)

    def count(self):
        path = self.home / "count"
        return len(path.read_bytes()) if path.exists() else 0

    def close(self, request_exit=True):
        if self.closed:
            return
        self.closed = True
        if request_exit:
            try:
                self.send(b"exit\n")
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

    def assert_workers_gone(self):
        for pid in self.workers:
            eventually(lambda: not active(pid), f"worker {pid} cleanup")


def run(binary: Path, output: Path):
    results = []
    shells = []
    def record(name, **details):
        result = {"test": name, "passed": True, **details}
        results.append(result)
        print(json.dumps(result), flush=True)
    def start(name, hold=False):
        shell = Shell(binary, output, name, hold)
        shells.append(shell)
        return shell
    try:
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
        os.kill(state["worker"], signal.SIGSTOP)
        shell.command("export CAP_BULK_A=${(l:95000::x:)} CAP_BULK_B=${(l:95000::y:)}")
        pending = shell.state()
        assert pending["tx"] > 0, pending
        assert pending["generation"] == 6
        os.kill(state["worker"], signal.SIGCONT)
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

        os.kill(current["worker"], signal.SIGKILL)
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
            if mode == "exec":
                shell.command("exec zsh -f -c 'exit 0'")
                shell.close(request_exit=False)
            else:
                shell.close()
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
    finally:
        for shell in shells:
            shell.close()
            for pid in shell.workers:
                if active(pid):
                    os.kill(pid, signal.SIGKILL)
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")


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
