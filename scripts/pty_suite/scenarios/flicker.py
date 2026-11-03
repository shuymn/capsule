"""Retained display across held acquisitions: local re-renders, replacement, invalidation, reload."""

from __future__ import annotations

from ..assertions import assert_history, display_width, external, rerender, wait_pending
from ..harness import SUBST_PROMPT, Gates, Prompt

ERROR_INPUT = b"\x1b[31m%}INPUT"
VICMD = b"\x1b[34m%}NORMAL"


def run_flicker(suite):
    with suite.session("flicker", flicker=True) as shell:
        shell.complete(1)
        assert shell.count() == 1
        old = {b"PTY_BRANCH_OLD", b"TOOL_OLD", b"FILE_OLD", b"COND_OLD"}
        shell.state()
        assert external(shell.rendered) == old
        gates = Gates(shell)
        try:
            with gates.stage("true") as prompt:
                assert_history(prompt, old)
            prompt.complete()
            assert_history(prompt, old, old)
            suite.record("ordinary_command_preserves_git_and_tools_in_every_render_and_precmd")

            # Change local duration deterministically without sleeping or exporting a gate.
            with gates.stage("_CAPSULE_CMD_START=$((EPOCHREALTIME-3)); false") as prompt:
                wait_pending(prompt, lambda text: b"DURATION" in text and b"3s" in text and ERROR_INPUT in text)
                assert_history(prompt, old)
                typed = b"echo cursor-marker"
                shell.type_text(typed, move_left=3)
                original = shell.assert_buffer(typed, cursor_offset=-3)
                wide = shell.rendered.split(b"\n")[0]
                narrow_state, _ = rerender(prompt, lambda: shell.resize(42), lambda text: text.split(b"\n")[0] != wide)
                assert narrow_state["cursor"] == original["cursor"]
                assert shell.buffer == typed
                shell.state()
                assert display_width(shell.rendered.split(b"\n")[0]) <= 42
                rerender(prompt, shell.enter_vicmd, lambda text: b"NORMAL" in text)

                def restore_wide_insert():
                    shell.enter_viins()
                    shell.resize(100)
                _, restore = rerender(prompt, restore_wide_insert, lambda text: external(text) == old and ERROR_INPUT in text)
                before = shell.state()
                assert external(shell.rendered) == old
                assert shell.buffer == typed
                gates.release()
                prompt.complete()
                after = shell.state()
                assert shell.buffer == typed
                assert before["cursor"] == after["cursor"]
                assert shell.count() == prompt.generation
                # Narrow-width truncation is intentional; every other render keeps
                # the settled external snapshot and uses the new local status.
                for text, _ in prompt.renders():
                    assert ERROR_INPUT in text or VICMD in text, text
                    observed = external(text)
                    assert observed <= old, text
                    if observed != old:
                        assert display_width(text.split(b"\n")[0]) <= 42, text
                    else:
                        assert b"DURATION" in text and b"3s" in text, text
                for transition in prompt.transitions():
                    if transition["event"] == b"apply":
                        assert transition["prompt"] == SUBST_PROMPT, transition
                # Receipt-time COLUMNS is not the render width: an in-flight narrow
                # response may arrive after expansion. Require full markers only
                # once a restored wide insert-mode prompt has actually been applied.
                restored_history = restore.transitions()
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
            suite.record("pending_acquisition_renders_status_duration_resize_and_vi_without_losing_input")

            shell.write("git.mode", "PTY_BRANCH_NEW")
            shell.write("tool.mode", "TOOL_NEW")
            shell.path("work/version").unlink()
            shell.path("work/condition").unlink()
            changed = {b"PTY_BRANCH_NEW", b"TOOL_NEW"}
            with gates.stage("true") as prompt:
                gates.release("git")
                gates.wait_done("git", "Git finishes before tool")
                rerender(prompt, lambda: shell.resize(99), lambda text: external(text) == old)
                assert_history(prompt, old)
                # Old/future responses must not replace the retained pending display.
                for wrong in (prompt.generation - 1, prompt.generation + 1):
                    shell.inject_frame(wrong, "wrong-generation", "wrong")
                    shell.state()
                assert_history(prompt, old)
            prompt.complete()
            assert_history(prompt, old, changed)
            shell.resize(100)
            suite.record("external_snapshot_commits_atomically_and_clears_missing_values_and_false_conditions")

            shell.write("tool.mode", "failed")
            with gates.stage("true") as prompt:
                # The previous stage injected this generation's completed frame while
                # it was still in the future. It remains in the raw log, not apply.
                whole_log = Prompt(shell, prompt.generation, (0, 0))
                assert any(complete and b"wrong-generation" in text for text, complete in whole_log.renders())
                try:
                    whole_log.complete(timeout=0)
                except AssertionError as error:
                    assert str(error) == f"deadline waiting for generation {prompt.generation} complete", error
                else:
                    raise AssertionError("rejected future frame satisfied pending completion")
                gates.release("tool")
                gates.wait_done("tool", "failed tool finishes before Git")
                rerender(prompt, lambda: shell.resize(99), lambda text: external(text) == changed)
                assert_history(prompt, changed)
            prompt.complete()
            failed = {b"PTY_BRANCH_NEW"}
            assert_history(prompt, changed, failed)
            suite.record("rejected_future_completion_does_not_satisfy_pending_generation")
            suite.record("failed_value_is_retained_pending_then_cleared_when_settled")

            shell.write("tool.mode", "TOOL_NEW")
            shell.path("work/other").mkdir()
            # Model directory-aware tools updating exports in a later precmd hook.
            with gates.stage(
                "_capsule_test_env_hook() { "
                "if [[ $PWD == $HOME/work/other ]]; then export CAP_FLICKER=exported; fi; "
                "return 0; }; "
                "add-zsh-hook precmd _capsule_test_env_hook; "
                "cd other"
            ) as prompt:
                assert_history(prompt, set())
            prompt.complete()
            exported = changed | {b"ENV=exported"}
            assert_history(prompt, set(), exported)
            suite.record("changed_cwd_discards_settled_external_display_immediately")
            suite.record("directory_hook_exports_reach_the_directory_change_generation")

            with gates.stage("true") as prompt:
                assert_history(prompt, exported)
            prompt.complete()
            assert_history(prompt, exported, exported)
            suite.record("first_command_after_directory_hook_update_retains_settled_display")

            with gates.stage("add-zsh-hook -d precmd _capsule_test_env_hook; unset CAP_FLICKER") as prompt:
                assert_history(prompt, set())
            prompt.complete()
            assert_history(prompt, set(), changed)
            with gates.stage("export CAP_FLICKER=exported") as prompt:
                assert_history(prompt, set())
            prompt.complete()
            assert_history(prompt, set(), exported)
            suite.record("changed_exported_environment_discards_settled_external_display_immediately")

            config = shell.config_path.read_text().replace("name = 'tool'\nformat = '{value}'", "name = 'tool'\nformat = 'RELOADED-{value}'")
            shell.write_config(config)
            reloaded = exported | {b"RELOADED-"}
            with gates.stage("true") as prompt:
                # The precmd may preserve the old display until asynchronous Plan load.
                wait_pending(prompt, lambda text: not external(text))
                transitions = prompt.transitions()
                cleared = next(index for index, item in enumerate(transitions) if item["event"] == b"apply" and not external(item["effective"]))
                assert all(not external(item["effective"]) for item in transitions[cleared:]), transitions
                # Keep the after-load boundary for all remaining pending/settled frames.
                rerender(prompt, lambda: shell.resize(100), lambda text: not external(text))
            prompt.complete()
            # Validate every frame/application across Plan load, not only the final prompt.
            cleared = False
            for text, complete in prompt.renders():
                observed = external(text)
                if complete:
                    assert observed == reloaded, (complete, text)
                else:
                    cleared = cleared or not observed
                    assert observed == (set() if cleared else exported), text
            cleared = False
            for item in prompt.transitions():
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
            suite.record("changed_valid_config_clears_display_after_plan_load_before_acquisition_finishes")

            shell.write_config("schema_version = [invalid\n")
            with gates.stage("true") as prompt:
                assert_history(prompt, reloaded)
            prompt.complete()
            assert_history(prompt, reloaded, reloaded)
            suite.record("invalid_config_retains_previous_valid_plan_and_settled_display")
            shell.write_config(config)
            suite.record("slow_fixtures_release_before_acquisition_deadline", max_release_seconds=max(item["release_seconds"] for item in gates.timings))
        finally:
            gates.close()
