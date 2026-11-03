"""Empty Enter is an ordinary prompt; repeated and nested-editor lines settle correctly."""

from __future__ import annotations

import subprocess

from .. import fixtures
from ..harness import Prompt


def run_empty_enter(suite):
    with suite.session("empty-enter") as shell:
        _empty_enter(shell, suite.record)


def _empty_enter(shell, record):
    shell.complete(1)
    worker = shell.state()["worker"]
    shell.write_config(fixtures.EMPTY_CONFIG)
    prompt = shell.run_line("git init -q && git symbolic-ref HEAD refs/heads/ENTER_BEFORE; _CAPSULE_CMD_START=$((EPOCHREALTIME-3)); (exit 7)")
    first = prompt.last_finalized()["duration"]
    shell.state()
    assert b"ENTER_BEFORE" in shell.rendered

    def assert_refreshed(prompt):
        state = shell.state()
        assert state["generation"] == prompt.generation and state["worker"] == worker, state
        rendered = shell.rendered
        assert b"ENTER_AFTER" in rendered and b"ENTER_BEFORE" not in rendered, rendered
        # Empty Enter is an ordinary prompt: zsh leaves $? unchanged and no command ran.
        finalized = prompt.last_finalized()
        assert finalized["status"] == 7 and finalized["duration"] == b"", finalized

    # Another terminal changes Git state; an empty line must show it.
    subprocess.run(
        ["git", "symbolic-ref", "HEAD", "refs/heads/ENTER_AFTER"],
        cwd=shell.path("work"), env={"HOME": str(shell.home), "PATH": "/usr/bin:/bin"}, check=True,
    )
    assert_refreshed(shell.run_line(""))
    record("empty_enter_refreshes_git_keeps_shell_status_and_omits_duration")

    # Repeated empty lines supersede unfinished work and settle on the newest generation.
    enters = 5
    generation = shell.next_generation() + enters - 1
    prompt = Prompt(shell, generation, shell.mark())
    shell.press_enter(enters)
    shell.ready(prompt.generation)
    prompt.complete()
    assert_refreshed(prompt)
    record("repeated_empty_enter_settles_on_the_newest_generation_with_one_worker", enters=enters)

    # The next command reports its own duration under the threshold rule again.
    finalized = shell.run_line("_CAPSULE_CMD_START=$((EPOCHREALTIME-2)); (exit 4)").last_finalized()
    # Measured from this command's start, not carried over from the first one.
    assert finalized["status"] == 4 and int(finalized["duration"]) >= 2000 and finalized["duration"] != first, finalized
    record("command_after_empty_enter_reports_its_own_status_and_duration")

    # A nested line editor inside the command must not discard its start time.
    prompt = shell.submit("_CAPSULE_CMD_START=$((EPOCHREALTIME-2)); vared -c _capsule_test_vared; (exit 6)")
    shell.press_enter()
    prompt.complete()
    finalized = prompt.last_finalized()
    assert finalized["status"] == 6 and int(finalized["duration"]) >= 2000, finalized
    record("command_using_nested_line_editor_keeps_its_duration")
