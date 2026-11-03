"""User precmd hooks: status, genuine shell errors, PROMPT_SUBST changes, and preloaded helpers."""

from __future__ import annotations

from dataclasses import dataclass
import signal

from .. import fixtures
from ..harness import FALLBACK_PROMPT, SUBST_PROMPT, eventually

INJECTION_MARKERS = ("dollar", "backtick")


@dataclass(frozen=True)
class MatrixCase:
    action: str  # "none", or "<hook|named>:<cd|export|setopt|error>"
    trigger: str  # "command" or "empty"

    @property
    def kind(self) -> str:
        return self.action.partition(":")[2]


MATRIX_ACTIONS = ("none", "hook:cd", "hook:export", "hook:setopt", "hook:error", "named:cd", "named:export", "named:setopt")
MATRIX_CASES = tuple(MatrixCase(action, trigger) for action in MATRIX_ACTIONS for trigger in ("command", "empty"))


def run_hook_matrix(suite):
    """Combine user-hook state changes with command and empty-Enter prompts."""
    with suite.session("hook-matrix", precmd_setup=fixtures.MATRIX_SETUP) as shell:
        shell.complete(1)
        worker = shell.state()["worker"]
        shell.path("work/matrix").mkdir()
        cwd, value, subst, count = shell.path("work"), fixtures.MATRIX_PAYLOAD.encode(), True, 0
        status = None
        for index, case in enumerate(MATRIX_CASES):
            shell.write("matrix-action", case.action)
            exit_code = 10 + index
            line = f"_CAPSULE_CMD_START=$((EPOCHREALTIME-3)); (exit {exit_code})" if case.trigger == "command" else ""
            prompt = shell.run_line(line, wait_ready=False)
            state = shell.state(wait_ready=False)
            assert state["generation"] == prompt.generation and state["worker"] == worker, (case, state)

            if case.kind == "cd":
                cwd = shell.path("work") if cwd != shell.path("work") else shell.path("work/matrix")
            elif case.kind == "export":
                count += 1
                value = fixtures.MATRIX_PAYLOAD.encode() + f" {count}".encode()
            elif case.kind == "setopt":
                subst = not subst
            # Acquisition sees cwd and exports after every hook ran or aborted.
            shell.assert_acquired_cwd(cwd)
            assert shell.acquired("value") == value, case

            finalized = prompt.last_finalized()
            if case.trigger == "command":
                assert finalized["status"] == exit_code and int(finalized["duration"]) >= 3000, (case, finalized)
                status = finalized["status"]
            else:
                assert (finalized["status"], finalized["duration"]) == (status, b""), (case, finalized)

            rendered = shell.rendered
            # Serialization escapes `$` and backticks; the command text must still be displayed.
            assert b"print -n x > ~/dollar" in rendered, (case, rendered)
            assert shell.prompt == (SUBST_PROMPT if subst else rendered), case
            shell.assert_no_injection(*INJECTION_MARKERS)
        suite.record("user_hook_changes_keep_snapshot_shell_status_and_literal_prompt_for_commands_and_empty_enter", cases=len(MATRIX_CASES))


@dataclass(frozen=True)
class ReacquisitionCase:
    command: str
    expected_branch: bytes
    expected_file_module: bytes | None  # None: the file module must be absent
    expected_untracked: bool


REACQUISITION_CASES = (
    ReacquisitionCase(
        command="git init -q && git symbolic-ref HEAD refs/heads/HOOK_BEFORE",
        expected_branch=b"HOOK_BEFORE", expected_file_module=None, expected_untracked=False,
    ),
    ReacquisitionCase(
        command="touch foo.md",
        expected_branch=b"HOOK_BEFORE", expected_file_module=b"FILE=", expected_untracked=True,
    ),
    ReacquisitionCase(
        command="git symbolic-ref HEAD refs/heads/HOOK_AFTER; print -r -- updated > foo.md",
        expected_branch=b"HOOK_AFTER", expected_file_module=b"FILE=updated", expected_untracked=True,
    ),
    ReacquisitionCase(
        command="rm foo.md",
        expected_branch=b"HOOK_AFTER", expected_file_module=None, expected_untracked=False,
    ),
)


def run_preloaded_hook_helper(suite):
    with suite.session("preloaded-hook-helper", precmd_setup=fixtures.PRELOADED_HOOK_SETUP) as shell:
        shell.complete(1)
        worker = shell.state()["worker"]
        shell.write_config(fixtures.FOO_FILE_CONFIG)
        # Use real Git and ordinary commands, not manually invoked Capsule hooks.
        for case in REACQUISITION_CASES:
            prompt = shell.run_line(case.command)
            state = shell.state()
            assert state["generation"] == prompt.generation and state["worker"] == worker, state
            effective = shell.effective_prompt
            assert case.expected_branch in effective, (case, effective)
            assert (b"[?]" in effective) == case.expected_untracked, (case, effective)
            if case.expected_file_module is None:
                assert b"FILE=" not in effective, (case, effective)
            else:
                assert case.expected_file_module in effective, (case, effective)
        suite.record("preloaded_add_zsh_hook_preserves_command_reacquisition_of_git_and_modules")


def run_hook_status(suite):
    with suite.session("hook-status", precmd_setup=fixtures.HOOK_STATUS_SETUP) as shell:
        shell.complete(1)
        shell.command("(exit 9)")
        shell.complete(2)
        assert shell.read("first-status") == b"9\n"
        assert shell.read("last-status") == b"9\n"
        assert shell.count() == 2
    suite.record("user_precmd_hooks_receive_original_command_status_after_nonzero_hook_returns")


def run_hook_errors(suite):
    with suite.session("hook-errors", precmd_setup=fixtures.HOOK_ERROR_SETUP) as shell:
        _hook_errors(shell, suite.record)


def _hook_errors(shell, record):
    shell.complete(1)
    shell.command("unsetopt promptsubst")
    shell.complete(2)
    worker = shell.state()["worker"]
    payload = b"$(>~/dollar) `>~/backtick`"
    displayed = payload + b"\ncharacter "

    # Withhold all new responses while a command changes PROMPT_SUBST and a
    # preserved hook raises a genuine shell error (not merely `return 1`).
    for generation, command in (
        (3, "_capsule_test_hook_error=1; setopt promptsubst; _CAPSULE_CMD_START=$((EPOCHREALTIME-3)); (exit 7)"),
        (5, "_capsule_test_hook_error=1; _capsule_test_hook_subst=on; _CAPSULE_CMD_START=$((EPOCHREALTIME-3)); (exit 7)"),
    ):
        if generation == 5:
            shell.command("_capsule_test_hook_error=0; unsetopt promptsubst")
            shell.complete(4)
        shell.signal_worker(worker, signal.SIGSTOP)
        shell.inject_frame(generation - 1, payload, "character")
        shell.state()
        assert shell.prompt == displayed
        shell.path("late-hook").unlink()
        prompt = shell.submit(command, generation=generation)
        eventually(
            lambda: any(item["event"] == b"precmd" for item in prompt.transitions()),
            "finalization after hook error", shell.pump,
        )
        state = shell.state(wait_ready=False)
        assert state["generation"] == generation and state["worker"] == worker
        assert shell.prompt == SUBST_PROMPT
        assert shell.rendered == displayed
        shell.assert_no_injection(*INJECTION_MARKERS)
        assert shell.exists("error-hook"), "failing user hook was not reached"
        assert not shell.exists("late-hook"), "hook error did not stop subsequent user hooks"
        shell.pump()
        assert b"division by zero" in shell.output()
        finalized = prompt.last_finalized()
        assert finalized["status"] == 7 and int(finalized["duration"]) >= 3000, finalized
        if generation == 3:
            shell.signal_worker(worker, signal.SIGCONT)
            prompt.complete()
    record("command_and_hook_promptsubst_transitions_remain_literal_after_genuine_hook_errors")
    record("hook_errors_skip_later_user_hooks_but_submit_generations_with_command_status_and_duration")

    shell.signal_worker(worker, signal.SIGKILL)
    eventually(lambda: shell.state(wait_ready=False)["read_fd"] == 0, "worker failure after hook error", shell.pump)
    assert shell.prompt == FALLBACK_PROMPT
    shell.path("work/other").mkdir()
    shell.submit("cd other; export CAP_VALUE=recovered", generation=6).complete()
    restarted = shell.state(wait_ready=False)
    assert restarted["generation"] == 6 and restarted["worker"] != worker
    assert shell.acquired("value") == b"recovered"
    shell.assert_acquired_cwd(shell.path("work/other"))
    assert not shell.exists("late-hook")
    record("hook_errors_do_not_prevent_snapshot_invalidation_or_replacement_worker_startup")
    shell.command("_capsule_test_hook_error=0")
    shell.complete(7)
    shell.ready(7)
    assert shell.exists("late-hook")


def run_named_hook_error(suite):
    with suite.session("named-hook-error", precmd_setup=fixtures.NAMED_HOOK_ERROR_SETUP) as shell:
        _named_hook_error(shell, suite.record)


def _named_hook_error(shell, record):
    # zsh runs a named precmd() before precmd_functions, so its genuine shell
    # error skips every Capsule hook before the worker-rendered prompt expands.
    payload = b"$(printf x > ~/dollar) `printf x > ~/backtick` \\"
    shell.complete(1)
    shell.write("work/data", payload)
    shell.write_config(fixtures.DATA_FILE_CONFIG)
    shell.command("unsetopt promptsubst")
    shell.complete(2)
    worker = shell.state()["worker"]
    retained = shell.prompt
    assert retained == shell.rendered, "option-off prompt must hold retained text"
    before = shell.output_offset()
    shell.command("_capsule_test_named_error=1; _CAPSULE_CMD_START=$((EPOCHREALTIME-3))")
    eventually(lambda: shell.exists("named-error"), "named precmd hook", shell.pump)
    eventually(lambda: b"division by zero" in shell.output(before), "genuine named hook error", shell.pump)
    state = shell.state(wait_ready=False)
    assert state["generation"] == 2 and state["worker"] == worker, "named hook error did not skip Capsule hooks"
    expanded = shell.output(before)
    assert payload in expanded[expanded.index(b"division by zero"):], "expanded prompt did not show the payload"
    assert shell.prompt == retained
    shell.assert_no_injection(*INJECTION_MARKERS)
    record("named_precmd_error_expands_worker_display_literally")

    # A matching response after the error still sees the option captured before it.
    line1 = shell.frames(2)[-1][0]
    mark = shell.mark()
    shell.inject_frame(2, line1, "injected")
    eventually(lambda: any(item["event"] == b"apply" for item in shell.transitions(mark, 2)), "late response", shell.pump)
    shell.state(wait_ready=False)
    rendered = shell.rendered
    assert rendered.endswith(b"\ninjected ") and shell.prompt == rendered
    shell.assert_no_injection(*INJECTION_MARKERS)
    record("response_after_named_precmd_error_stays_literal")

    # The skipped prompt never consumed the command start; the next prompt must
    # not attribute it, plus the time spent at that prompt, to empty Enter.
    prompt = shell.run_line("")
    finalized = prompt.finalized()
    assert prompt.generation == 3 and finalized and finalized[-1]["duration"] == b"", finalized
    record("empty_enter_after_named_precmd_error_omits_stale_duration")

    finalized = shell.run_line("_CAPSULE_CMD_START=$((EPOCHREALTIME-2)); (exit 7)").last_finalized()
    assert finalized["status"] == 7 and int(finalized["duration"]) >= 2000, finalized
    assert shell.state()["worker"] == worker
    shell.assert_no_injection(*INJECTION_MARKERS)
    record("named_precmd_recovery_keeps_command_status_and_worker")
