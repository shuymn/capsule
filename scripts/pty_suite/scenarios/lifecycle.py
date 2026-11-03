"""Binary selection, worker ownership, snapshots, transport, and cleanup."""

from __future__ import annotations

from contextlib import nullcontext
import errno
import os
import signal
import subprocess

from .. import fixtures
from ..harness import FALLBACK_PROMPT, Fifo, active, eventually, live_replacement


def run_binary_selection(suite):
    # A renamed selected executable must win over a conflicting `capsule`.
    binary_fixture = suite.output / "binary-selection-bin"
    binary_fixture.mkdir()
    candidate = binary_fixture / "capsule-candidate"
    candidate.symlink_to(suite.binary)
    conflicting = binary_fixture / "capsule"
    conflicting.write_text(fixtures.CONFLICTING_CAPSULE)
    conflicting.chmod(0o755)
    with suite.session("binary-selection", selected_binary=candidate) as shell:
        shell.complete(1)
        assert shell.state()["worker"] and shell.count() == 1
        assert not shell.exists("wrong-binary")
    suite.record("renamed_selected_binary_provides_init_worker_and_fd_config")

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
    suite.record("retired_worker_pids_are_not_cleanup_targets")


def run_functional(suite):
    with suite.session("functional") as shell:
        shell.complete(1)
        original = shell.state()
        assert shell.count() == 1
        assert b"VALUE=initial" in shell.rendered
        suite.record("one_shell_starts_one_worker", worker=original["worker"])

        shell.resize(70)
        eventually(lambda: shell.state()["cols"] == 70, "resize request", shell.pump)
        shell.enter_vicmd()
        eventually(lambda: shell.state()["keymap"] == "vicmd", "vi command mode", shell.pump)
        eventually(lambda: shell.state() and "❮".encode() in shell.rendered, "vi glyph", shell.pump)
        shell.enter_viins()
        eventually(lambda: shell.state()["keymap"] in ("main", "viins"), "vi insert mode", shell.pump)
        assert shell.count() == 1
        suite.record("resize_and_keymap_rerender_without_acquisition")

        shell.run_line("unset CAP_REMOVE; CAP_PRIVATE=private; export CAP_VALUE=$'日本語\\tline\\n\\\\tail'; export CAP_RAW=$'raw-\\xff'")
        assert shell.read("value") == "日本語\tline\n\\tail".encode()
        assert shell.read("raw") == b"raw-\xff"
        assert shell.read("removed") == b"unset"
        assert shell.read("private") == b"unset"
        suite.record("full_export_snapshot_preserves_bytes_and_removes_absent_variables")

        raw_cwd = os.fsencode(shell.path("work")) + b"/raw-\xff"
        non_utf8 = True
        try:
            os.mkdir(raw_cwd)
            source = "cd $'raw-\\xff'"
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            # APFS can reject invalid UTF-8 names; Linux exercises that boundary.
            non_utf8 = False
            directory = shell.path("work") / "日本語 cwd"
            directory.mkdir()
            raw_cwd = os.fsencode(directory)
            source = "cd '日本語 cwd'"
        shell.run_line(source)
        assert shell.acquired_cwd == raw_cwd
        suite.record("cwd_bytes_reach_acquisition", non_utf8_fixture_supported=non_utf8)

        gate = Fifo(shell.path("gate"))
        prompt = shell.submit("export CAP_GATE=1 CAP_VALUE=after-gate")
        eventually(lambda: shell.exists("gate-ready"), "gated acquisition", shell.pump)
        shell.ready(prompt.generation)
        typed = b"echo cursor-marker"
        shell.type_text(typed, move_left=3)
        shell.assert_buffer(typed, cursor_offset=-3)
        gate.release()
        gate.close()
        prompt.complete()
        shell.assert_buffer(typed, cursor_offset=-3)
        assert b"VALUE=after-gate" in shell.rendered
        suite.record("asynchronous_prompt_update_preserves_typed_buffer_and_cursor")
        shell.clear_line()
        shell.run_line("unset CAP_GATE; export CAP_VALUE=bulk")

        state = shell.state()
        shell.signal_worker(state["worker"], signal.SIGSTOP)
        prompt = shell.submit("export CAP_BULK_A=${(l:95000::x:)} CAP_BULK_B=${(l:95000::y:)}", generation=state["generation"] + 1)
        pending = shell.state()
        assert pending["tx"] > 0, pending
        assert pending["generation"] == prompt.generation
        shell.signal_worker(state["worker"], signal.SIGCONT)
        # No keyboard input until the worker finishes: credit notifications must progress TX.
        prompt.complete()
        drained = shell.state()
        assert drained["tx"] == drained["pending"] == 0
        suite.record("backpressure_completes_large_request_without_more_keyboard_input", queued_bytes=pending["tx"])

        current = shell.state()
        displayed = shell.rendered
        for generation in (current["generation"] - 1, current["generation"] + 1):
            shell.inject_frame(generation, "wrong-generation", "wrong")
            shell.state()
            assert shell.rendered == displayed
        suite.record("stale_and_future_responses_do_not_replace_prompt")

        shell.signal_worker(current["worker"], signal.SIGKILL)
        eventually(lambda: shell.state()["read_fd"] == 0, "worker failure fallback", shell.pump)
        assert shell.prompt == FALLBACK_PROMPT
        shell.run_line("unset CAP_BULK_A CAP_BULK_B")
        assert shell.state()["worker"] != current["worker"]
        suite.record("worker_failure_uses_fallback_and_next_command_restarts_worker")


def run_cleanup(suite):
    _assert_session_cleanup(suite, "exit")
    suite.record("exit_closes_cloexec_pipes_and_cleans_active_process_group")
    _assert_session_cleanup(suite, "exec")
    suite.record("exec_closes_cloexec_pipes_and_cleans_active_process_group")


def _assert_session_cleanup(suite, mode):
    shell = suite.start("cleanup-" + mode, hold=True)
    eventually(lambda: shell.exists("children"), "active acquisition descendants", shell.pump)
    shell.state()
    children = list(map(int, shell.read("children").split()))
    if mode == "exit":
        shell.close()
    with (live_replacement(shell) if mode == "exec" else nullcontext()):
        shell.assert_workers_gone()
        for pid in children:
            eventually(lambda pid=pid: not active(pid), "acquisition descendant cleanup")


def run_parallel(suite):
    many = [suite.start(f"parallel-{index}") for index in range(10)]
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
    suite.record("ten_shells_own_ten_workers_and_leave_none_active")
