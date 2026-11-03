"""Retained display across held acquisitions: local re-renders, replacement, invalidation, reload."""

from __future__ import annotations

from ..assertions import assert_history, display_width, external, wait_pending
from ..harness import SUBST_PROMPT, Gates

ERROR_INPUT = b"\x1b[31m%}INPUT"
VICMD = b"\x1b[34m%}NORMAL"


def run_flicker(suite):
    with suite.session("flicker", flicker=True) as shell:
        _flicker(shell, suite.record)


def _flicker(shell, record):
    shell.complete(1)
    assert shell.count() == 1
    old = {b"PTY_BRANCH_OLD", b"TOOL_OLD", b"FILE_OLD", b"COND_OLD"}
    shell.state()
    assert external(shell.rendered) == old
    gates = Gates(shell)
    try:
        with gates.stage("true") as (mark, generation):
            assert_history(shell, mark, generation, old)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, old, old)
        record("ordinary_command_preserves_git_and_tools_in_every_render_and_precmd")

        # Change local duration deterministically without sleeping or exporting a gate.
        with gates.stage("_CAPSULE_CMD_START=$((EPOCHREALTIME-3)); false") as (mark, generation):
            wait_pending(shell, mark, generation, lambda prompt: b"DURATION" in prompt and b"3s" in prompt and ERROR_INPUT in prompt)
            assert_history(shell, mark, generation, old)
            typed = b"echo cursor-marker"
            shell.type_text(typed, move_left=3)
            original = shell.assert_buffer(typed, cursor_offset=-3)
            wide = shell.rendered.split(b"\n")[0]
            redraw = shell.mark()
            shell.resize(42)
            narrow_state = shell.state()  # Trigger zsh's line-pre-redraw hook.
            wait_pending(shell, redraw, generation, lambda prompt: prompt.split(b"\n")[0] != wide)
            assert narrow_state["cursor"] == original["cursor"]
            assert shell.buffer == typed
            shell.state()
            assert display_width(shell.rendered.split(b"\n")[0]) <= 42
            redraw = shell.mark()
            shell.enter_vicmd()
            shell.state()
            wait_pending(shell, redraw, generation, lambda prompt: b"NORMAL" in prompt)
            restore = shell.mark()
            shell.enter_viins()
            shell.resize(100)
            shell.state()
            wait_pending(shell, restore, generation, lambda prompt: external(prompt) == old and ERROR_INPUT in prompt)
            before = shell.state()
            assert external(shell.rendered) == old
            assert shell.buffer == typed
            gates.release()
            shell.complete(generation, mark)
            after = shell.state()
            assert shell.buffer == typed
            assert before["cursor"] == after["cursor"]
            assert shell.count() == generation
            # Narrow-width truncation is intentional; every other render keeps
            # the settled external snapshot and uses the new local status.
            for prompt, _ in shell.renders(mark, generation):
                assert ERROR_INPUT in prompt or VICMD in prompt, prompt
                observed = external(prompt)
                assert observed <= old, prompt
                if observed != old:
                    assert display_width(prompt.split(b"\n")[0]) <= 42, prompt
                else:
                    assert b"DURATION" in prompt and b"3s" in prompt, prompt
            for transition in shell.transitions(mark, generation):
                if transition["event"] == b"apply":
                    assert transition["prompt"] == SUBST_PROMPT, transition
            # Receipt-time COLUMNS is not the render width: an in-flight narrow
            # response may arrive after expansion. Require full markers only
            # once a restored wide insert-mode prompt has actually been applied.
            restored_history = shell.transitions(restore, generation)
            wide_restored = False
            for transition in restored_history:
                if (transition["event"] == b"apply"
                        and external(transition["effective"]) == old
                        and ERROR_INPUT in transition["effective"]):
                    wide_restored = True
                if wide_restored:
                    assert external(transition["effective"]) == old, transition
            assert wide_restored, restored_history
        shell.clear_line()
        record("pending_acquisition_renders_status_duration_resize_and_vi_without_losing_input")

        shell.write("git.mode", "PTY_BRANCH_NEW")
        shell.write("tool.mode", "TOOL_NEW")
        shell.path("work/version").unlink()
        shell.path("work/condition").unlink()
        changed = {b"PTY_BRANCH_NEW", b"TOOL_NEW"}
        with gates.stage("true") as (mark, generation):
            gates.release("git")
            gates.wait_done("git", "Git finishes before tool")
            redraw = shell.mark()
            shell.resize(99)
            shell.state()
            wait_pending(shell, redraw, generation, lambda prompt: external(prompt) == old)
            assert_history(shell, mark, generation, old)
            # Old/future responses must not replace the retained pending display.
            for wrong in (generation - 1, generation + 1):
                shell.inject_frame(wrong, "wrong-generation", "wrong")
                shell.state()
            assert_history(shell, mark, generation, old)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, old, changed)
        shell.resize(100)
        record("external_snapshot_commits_atomically_and_clears_missing_values_and_false_conditions")

        shell.write("tool.mode", "failed")
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
            gates.wait_done("tool", "failed tool finishes before Git")
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

        shell.write("tool.mode", "TOOL_NEW")
        shell.path("work/other").mkdir()
        # Model directory-aware tools updating exports in a later precmd hook.
        with gates.stage(
            "_capsule_test_env_hook() { "
            "if [[ $PWD == $HOME/work/other ]]; then export CAP_FLICKER=exported; fi; "
            "return 0; }; "
            "add-zsh-hook precmd _capsule_test_env_hook; "
            "cd other"
        ) as (mark, generation):
            assert_history(shell, mark, generation, set())
        shell.complete(generation, mark)
        exported = changed | {b"ENV=exported"}
        assert_history(shell, mark, generation, set(), exported)
        record("changed_cwd_discards_settled_external_display_immediately")
        record("directory_hook_exports_reach_the_directory_change_generation")

        with gates.stage("true") as (mark, generation):
            assert_history(shell, mark, generation, exported)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, exported, exported)
        record("first_command_after_directory_hook_update_retains_settled_display")

        with gates.stage("add-zsh-hook -d precmd _capsule_test_env_hook; unset CAP_FLICKER") as (mark, generation):
            assert_history(shell, mark, generation, set())
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, set(), changed)
        with gates.stage("export CAP_FLICKER=exported") as (mark, generation):
            assert_history(shell, mark, generation, set())
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, set(), exported)
        record("changed_exported_environment_discards_settled_external_display_immediately")

        config = shell.config_path.read_text().replace("name = 'tool'\nformat = '{value}'", "name = 'tool'\nformat = 'RELOADED-{value}'")
        shell.write_config(config)
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
        assert external(shell.rendered) == reloaded
        record("changed_valid_config_clears_display_after_plan_load_before_acquisition_finishes")

        shell.write_config("schema_version = [invalid\n")
        with gates.stage("true") as (mark, generation):
            assert_history(shell, mark, generation, reloaded)
        shell.complete(generation, mark)
        assert_history(shell, mark, generation, reloaded, reloaded)
        record("invalid_config_retains_previous_valid_plan_and_settled_display")
        shell.write_config(config)
        record("slow_fixtures_release_before_acquisition_deadline", max_release_seconds=max(item["release_seconds"] for item in gates.timings))
    finally:
        gates.close()
