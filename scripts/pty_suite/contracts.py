"""Registry of the interactive shell contract: each scope, its rule, and its records.

docs/testing.md embeds `markdown_table()` between markers (rewrite it with
`session_pty.py --update-docs`), and the driver
rejects records that no contract lists and fails when a listed record is missing.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Contract:
    scope: str
    behavior: str
    records: tuple[str, ...]  # results.json test names that witness the behavior


# Rows whose rule is a docs/architecture.md clause, which cites their records.
CLAUSE = "The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records"

CONTRACTS = (
    Contract(
        "Binary selection",
        "A renamed selected executable supplies init, worker, and fd-config even beside a different executable named capsule",
        ("renamed_selected_binary_provides_init_worker_and_fd_config",),
    ),
    Contract(
        "Cleanup safety",
        "Confirmed-dead worker identities are retired; historical PIDs never become signal targets after reuse",
        ("retired_worker_pids_are_not_cleanup_targets",),
    ),
    Contract(
        "One shell",
        "One worker; resize/keymap changes reuse acquisition",
        ("one_shell_starts_one_worker", "resize_and_keymap_rerender_without_acquisition"),
    ),
    Contract(
        "Empty Enter",
        "Empty Enter is an ordinary prompt: it shows Git changes made outside the shell, keeps zsh's unchanged `$?` as the status, "
        "and omits the previous command's duration; the next command, including one using a nested line editor, shows its own duration; "
        "repeated empty Enter settles on the newest generation with the same worker",
        (
            "empty_enter_refreshes_git_keeps_shell_status_and_omits_duration",
            "repeated_empty_enter_settles_on_the_newest_generation_with_one_worker",
            "command_after_empty_enter_reports_its_own_status_and_duration",
            "command_using_nested_line_editor_keeps_its_duration",
        ),
    ),
    Contract(
        "Hook matrix",
        "For commands and empty Enter combined with named or preserved hooks that change cwd, exports, or `PROMPT_SUBST`, "
        "or raise a preserved-hook shell error: each prompt advances one generation on the same worker, acquisition sees post-hook cwd/exports, "
        "status follows zsh's `$?`, duration appears only after a command, and the prompt stays literal for the current option",
        ("user_hook_changes_keep_snapshot_shell_status_and_literal_prompt_for_commands_and_empty_enter",),
    ),
    Contract(
        "Initialization",
        "With `add-zsh-hook` already loaded by another integration, ordinary commands advance generations and update real Git state "
        "and file-backed modules using the same worker",
        ("preloaded_add_zsh_hook_preserves_command_reacquisition_of_git_and_modules",),
    ),
    Contract(
        "Hook status",
        "Every preserved user precmd hook receives the command's original status, even after an earlier hook returns nonzero",
        ("user_precmd_hooks_receive_original_command_status_after_nonzero_hook_returns",),
    ),
    Contract(
        "Hook errors",
        "When a preserved precmd hook raises a genuine shell error, later user hooks are skipped, the generation is still submitted "
        "with the command's status and duration, and the retained display stays literal across command- and hook-level `PROMPT_SUBST` changes",
        (
            "command_and_hook_promptsubst_transitions_remain_literal_after_genuine_hook_errors",
            "hook_errors_skip_later_user_hooks_but_submit_generations_with_command_status_and_duration",
        ),
    ),
    Contract(
        "Revalidation",
        "With file-controlled slow acquisition released before the deadline and unchanged cwd/environment/config, ordinary commands "
        "preserve Git/tool information in every intermediate frame and actual shell prompt assignment, including finalized precmd; "
        "the first command after a directory-aware precmd hook updates exports also preserves the settled display",
        (
            "ordinary_command_preserves_git_and_tools_in_every_render_and_precmd",
            "first_command_after_directory_hook_update_retains_settled_display",
            "slow_fixtures_release_before_acquisition_deadline",
        ),
    ),
    Contract(
        "Replacement",
        CLAUSE,
        (
            "external_snapshot_commits_atomically_and_clears_missing_values_and_false_conditions",
            "failed_value_is_retained_pending_then_cleared_when_settled",
        ),
    ),
    Contract(
        "Invalidation",
        CLAUSE,
        (
            "changed_cwd_discards_settled_external_display_immediately",
            "changed_exported_environment_discards_settled_external_display_immediately",
            "changed_valid_config_clears_display_after_plan_load_before_acquisition_finishes",
            "invalid_config_retains_previous_valid_plan_and_settled_display",
        ),
    ),
    Contract(
        "Local display",
        CLAUSE,
        ("pending_acquisition_renders_status_duration_resize_and_vi_without_losing_input",),
    ),
    Contract(
        "Snapshot",
        CLAUSE,
        (
            "full_export_snapshot_preserves_bytes_and_removes_absent_variables",
            "cwd_bytes_reach_acquisition",
            "directory_hook_exports_reach_the_directory_change_generation",
        ),
    ),
    Contract(
        "Input",
        CLAUSE,
        ("asynchronous_prompt_update_preserves_typed_buffer_and_cursor",),
    ),
    Contract(
        "Transport",
        CLAUSE,
        (
            "backpressure_completes_large_request_without_more_keyboard_input",
            "stale_and_future_responses_do_not_replace_prompt",
            "rejected_future_completion_does_not_satisfy_pending_generation",
        ),
    ),
    Contract(
        "Recovery",
        CLAUSE,
        (
            "worker_failure_uses_fallback_and_next_command_restarts_worker",
            "hook_errors_do_not_prevent_snapshot_invalidation_or_replacement_worker_startup",
        ),
    ),
    Contract(
        "Named hook",
        "After a named `precmd` enables `PROMPT_SUBST` and raises a genuine shell error, the expanded worker display and a later matching "
        "response stay literal without marker side effects; the following empty Enter omits the skipped command's duration; "
        "the next command keeps its status and worker",
        (
            "named_precmd_error_expands_worker_display_literally",
            "response_after_named_precmd_error_stays_literal",
            "empty_enter_after_named_precmd_error_omits_stale_duration",
            "named_precmd_recovery_keeps_command_status_and_worker",
        ),
    ),
    Contract(
        "Exit and exec",
        "Pipes close; worker and active acquisition descendants terminate; exec cleanup is checked after a replacement-ready marker "
        "and while the controlled replacement remains alive",
        (
            "exit_closes_cloexec_pipes_and_cleans_active_process_group",
            "exec_closes_cloexec_pipes_and_cleans_active_process_group",
        ),
    ),
    Contract(
        "Ten shells",
        "Ten distinct workers, one initial acquisition each, and no workers left active after exit",
        ("ten_shells_own_ten_workers_and_leave_none_active",),
    ),
)

RECORDS = frozenset(name for contract in CONTRACTS for name in contract.records)

TABLE_BEGIN = "<!-- pty-contract:begin -->"
TABLE_END = "<!-- pty-contract:end -->"


def markdown_table() -> str:
    lines = ["| Scope | Required behavior | `results.json` records |", "|---|---|---|"]
    for contract in CONTRACTS:
        records = "<br>".join(f"`{name}`" for name in contract.records)
        lines.append(f"| {contract.scope} | {contract.behavior} | {records} |")
    return "\n".join(lines) + "\n"


def update_document(text: str) -> str:
    """Replace the table between TABLE_BEGIN and TABLE_END with `markdown_table()`."""
    head, rest = text.split(TABLE_BEGIN + "\n", 1)
    _, tail = rest.split(TABLE_END, 1)
    return head + TABLE_BEGIN + "\n" + markdown_table() + TABLE_END + tail


def missing_records(recorded) -> list[str]:
    """Registered records absent from `recorded`, in registry order."""
    recorded = set(recorded)
    return [name for contract in CONTRACTS for name in contract.records if name not in recorded]
