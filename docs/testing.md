# Testing

Read this file before writing or modifying tests.

## Suite and conventions

- Use `mise run test` for PTY-driver CLI regressions, unit/integration targets, and
  doctests. The CLI regressions require uv but no built Capsule binary or PTY.
  uv runs every Python script with the version pinned in
  [.python-version](../.python-version), so local, CI, and Docker runs match. Cargo's `--all-targets` does not include doctests; the task runs them
  separately.
- Use `cargo test -p <crate> <filter> --locked` for focused checks. See
  [tooling.md](tooling.md) for full verification and Git-hook side effects.
- Put unit tests in a final `#[cfg(test)] mod tests`, inline or in `tests.rs`.
  Put public API and CLI integration tests under `tests/` and shared helpers in
  `tests/common/mod.rs`, which Cargo does not treat as a separate test binary.
- Use isolated fixtures, `Result` with `?` for fallible setup, and RAII cleanup.
  Assert behavior, including relevant failure/cancellation paths, without depending
  on execution order. Explain the reason and removal condition for any ignored test.
- State invariants over generated inputs with `proptest` where a contract holds for
  every input (round trips, bounds, absence of panics). Keep the default case count,
  and commit any `proptest-regressions/` file a failure writes so the shrunk case
  stays in the suite.
- [docs.rs](../crates/cli/tests/docs.rs) keeps documentation that restates code
  facts in sync: relative links and heading anchors (ATX headings only;
  emphasis and HTML in headings fail), backticked identifiers (snake case,
  screaming case, or `::` paths; a method path, even a fully qualified one
  such as `capsule_core::ConfigPlan::parse`, must be defined in its type's
  `impl`), the limits table, the lint policy, lint
  inheritance in every workspace member, and that each Behavioral contract
  clause cites a test, clause check, or PTY record. Fix the document or the code when it
  fails; list a name that is intentionally not defined here in the test's
  external-identifier list with its reason.
- Check worker state decisions in [worker/machine/tests.rs](../crates/cli/src/worker/machine/tests.rs):
  a reference model written from the behavioral contract, plus the clause
  checks that architecture.md cites, runs against random interleavings of
  requests, events, and task completion. Change the model with the contract,
  not to match the implementation.
- Tag a test that verifies a threat in [threat-model.md](threat-model.md) with
  a `Covers: T07, T08` comment directly above its attributes, or in a shell
  script's leading comments to cover the whole file. A Rust tag needs a
  `#[test]` or `#[tokio::test]` attribute, and a PTY tag a function in
  `SCENARIOS`.
  `docs.rs` rejects unknown threat IDs and stray tags and checks the §8
  "Existing verification" column; regenerate it with
  `CAPSULE_UPDATE_DOCS=1 cargo test -p capsule-cli --test docs`.
- Measure test strength with `mise run test:mutants` ([cargo-mutants](https://mutants.rs/),
  configured in [.cargo/mutants.toml](../.cargo/mutants.toml)). It is opt-in and slow,
  so pass `--file <path>` or `--in-diff <diff-file>` for the code you changed. A missed
  mutant is a test gap: add the case that kills it. For an equivalent mutant, prefer
  code that cannot produce it; otherwise exclude it there with a comment.
- Fuzz parsers of untrusted bytes with [bolero](https://camshaft.github.io/bolero/)
  harnesses. `cargo test` runs each for one second plus its corpus; fuzz one with
  `mise run test:fuzz <package> <test path> -T 10m`, and the weekly
  [fuzz workflow](../.github/workflows/fuzz.yml) runs all of them. Commit only
  hand-written `seed-*` files in `__fuzz__/<target>/corpus/`; to keep a crash from
  `__fuzz__/<target>/crashes/` as a regression, copy it there as a seed.
- Keep doctests runnable; use hidden `# ` setup, `no_run` for external resources,
  and `compile_fail` for invalid usage.

## Interactive shell contract

Run [scripts/session_pty.py](../scripts/session_pty.py) on both macOS and Linux when
changing shell integration, worker transport, or acquisition lifecycle. It requires
zsh, uv, Git, and `ps`, uses real PTYs and disposable homes, and leaves user shell
configuration untouched. CI runs it on macOS and Linux; run it locally on the
target OS from the repository root, or in the Linux image with `mise run docker:linux:check`:

```sh
mise run test:pty
```

The default binary is `target/debug/capsule`; use `--binary <path>` for another
build, including a renamed executable. Require that exact build for `init`,
`worker`, and `fd-config`: the driver puts a disposable `capsule` symlink first on
each shell's PATH and probes a renamed target beside a conflicting `capsule`.
The script prints its artifact directory containing `results.json` and each shell's
`terminal.log`. Use `--output <new-directory>` to choose a fresh destination.

- WHEN `--output` names a new directory, the driver SHALL create it exclusively
  with mode `0700` (or stricter under the caller's umask).
- WHEN `--output` names an existing directory, file, or symlink, the driver SHALL
  reject it without changing its contents or target.
- WHEN `--output` is omitted, the driver SHALL keep the private
  `tempfile.mkdtemp()` default.

Use a trusted or sticky parent directory whose other users cannot replace the
new output entry. Private output permissions do not protect against an attacker
controlling its parent or arbitrary same-UID code. Run the CLI regressions independently with
`uv run --no-project python -m unittest discover -s scripts -p test_session_pty.py`.

The flicker setup directly launches each fresh Git/tool executable once before
starting zsh, with a separate 8s timeout, EOF stdin, disposable `HOME`, cwd
`$HOME/work`, and fixture bin first on `PATH`. Require successful exit, exact
output, and done markers. Verify the setup-only tool count is exactly one, then
remove the count and both done markers before shell startup. Keep the production
500ms deadline, generation counts, accepted-completion witnesses, and gate
deadlines unchanged; do not retry failed scenarios.

Require successful exit and all contract records. [contracts.py](../scripts/pty_suite/contracts.py) is the source of this table: the driver rejects a record no contract lists and fails when a listed record is missing, and the CLI regressions fail when the table below differs from the registry. Edit the registry, then run `uv run --no-project scripts/session_pty.py --update-docs`:

<!-- pty-contract:begin -->
| Scope | Required behavior | `results.json` records |
|---|---|---|
| Binary selection | A renamed selected executable supplies init, worker, and fd-config even beside a different executable named capsule | `renamed_selected_binary_provides_init_worker_and_fd_config` |
| Cleanup safety | Confirmed-dead worker identities are retired; historical PIDs never become signal targets after reuse | `retired_worker_pids_are_not_cleanup_targets` |
| One shell | One worker; resize/keymap changes reuse acquisition | `one_shell_starts_one_worker`<br>`resize_and_keymap_rerender_without_acquisition` |
| Empty Enter | Empty Enter is an ordinary prompt: it shows Git changes made outside the shell, keeps zsh's unchanged `$?` as the status, and omits the previous command's duration; the next command, including one using a nested line editor, shows its own duration; repeated empty Enter settles on the newest generation with the same worker | `empty_enter_refreshes_git_keeps_shell_status_and_omits_duration`<br>`repeated_empty_enter_settles_on_the_newest_generation_with_one_worker`<br>`command_after_empty_enter_reports_its_own_status_and_duration`<br>`command_using_nested_line_editor_keeps_its_duration` |
| Hook matrix | For commands and empty Enter combined with named or preserved hooks that change cwd, exports, or `PROMPT_SUBST`, or raise a preserved-hook shell error: each prompt advances one generation on the same worker, acquisition sees post-hook cwd/exports, status follows zsh's `$?`, duration appears only after a command, and the prompt stays literal for the current option | `user_hook_changes_keep_snapshot_shell_status_and_literal_prompt_for_commands_and_empty_enter` |
| Initialization | With `add-zsh-hook` already loaded by another integration, ordinary commands advance generations and update real Git state and file-backed modules using the same worker | `preloaded_add_zsh_hook_preserves_command_reacquisition_of_git_and_modules` |
| Hook status | Every preserved user precmd hook receives the command's original status, even after an earlier hook returns nonzero | `user_precmd_hooks_receive_original_command_status_after_nonzero_hook_returns` |
| Hook errors | When a preserved precmd hook raises a genuine shell error, later user hooks are skipped, the generation is still submitted with the command's status and duration, and the retained display stays literal across command- and hook-level `PROMPT_SUBST` changes | `command_and_hook_promptsubst_transitions_remain_literal_after_genuine_hook_errors`<br>`hook_errors_skip_later_user_hooks_but_submit_generations_with_command_status_and_duration` |
| Revalidation | With file-controlled slow acquisition released before the deadline and unchanged cwd/environment/config, ordinary commands preserve Git/tool information in every intermediate frame and actual shell prompt assignment, including finalized precmd; the first command after a directory-aware precmd hook updates exports also preserves the settled display | `ordinary_command_preserves_git_and_tools_in_every_render_and_precmd`<br>`first_command_after_directory_hook_update_retains_settled_display`<br>`slow_fixtures_release_before_acquisition_deadline` |
| Replacement | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `external_snapshot_commits_atomically_and_clears_missing_values_and_false_conditions`<br>`failed_value_is_retained_pending_then_cleared_when_settled` |
| Invalidation | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `changed_cwd_discards_settled_external_display_immediately`<br>`changed_exported_environment_discards_settled_external_display_immediately`<br>`changed_valid_config_clears_display_after_plan_load_before_acquisition_finishes`<br>`invalid_config_retains_previous_valid_plan_and_settled_display` |
| Local display | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `pending_acquisition_renders_status_duration_resize_and_vi_without_losing_input` |
| Snapshot | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `full_export_snapshot_preserves_bytes_and_removes_absent_variables`<br>`cwd_bytes_reach_acquisition`<br>`directory_hook_exports_reach_the_directory_change_generation` |
| Input | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `asynchronous_prompt_update_preserves_typed_buffer_and_cursor` |
| Transport | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `backpressure_completes_large_request_without_more_keyboard_input`<br>`stale_and_future_responses_do_not_replace_prompt`<br>`rejected_future_completion_does_not_satisfy_pending_generation` |
| Recovery | The [Behavioral contract](architecture.md#behavioral-contract) clause citing these records | `worker_failure_uses_fallback_and_next_command_restarts_worker`<br>`hook_errors_do_not_prevent_snapshot_invalidation_or_replacement_worker_startup` |
| Named hook | After a named `precmd` enables `PROMPT_SUBST` and raises a genuine shell error, the expanded worker display and a later matching response stay literal without marker side effects; the following empty Enter omits the skipped command's duration; the next command keeps its status and worker | `named_precmd_error_expands_worker_display_literally`<br>`response_after_named_precmd_error_stays_literal`<br>`empty_enter_after_named_precmd_error_omits_stale_duration`<br>`named_precmd_recovery_keeps_command_status_and_worker` |
| Exit and exec | Pipes close; worker and active acquisition descendants terminate; exec cleanup is checked after a replacement-ready marker and while the controlled replacement remains alive | `exit_closes_cloexec_pipes_and_cleans_active_process_group`<br>`exec_closes_cloexec_pipes_and_cleans_active_process_group` |
| Ten shells | Ten distinct workers, one initial acquisition each, and no workers left active after exit | `ten_shells_own_ten_workers_and_leave_none_active` |
<!-- pty-contract:end -->

Write scenarios in [pty_suite/scenarios/](../scripts/pty_suite/scenarios) with the `Shell` operations in [harness.py](../scripts/pty_suite/harness.py) (`type_text`, `enter_vicmd`, `inject_frame`, `rendered`, `assert_buffer`, ...) instead of raw control codes or probe files, keep generated shell and config text in [fixtures.py](../scripts/pty_suite/fixtures.py), and add each new scenario to the run order in `SCENARIOS`. Every prompt that reaches Capsule finalization starts a generation, so derive expected generations with `Shell.submit()` or `Shell.run_line()`, which return the generation and its log mark, instead of hard-coding counts. A genuine error in a named `precmd` skips Capsule's hooks and leaves the generation unchanged; assert that case directly.

Record each platform separately. A filesystem that rejects non-UTF-8 names produces
`non_utf8_fixture_supported = false`; require the Linux record with `true` for that
cwd boundary. Partial `results.json` output from a failed run is not a pass.

Also run `mise run test` on each platform.
Suite results and acquisition timings do not replace interactive PTY evidence. Follow
[benchmarking.md](benchmarking.md) for timing definitions and host requirements.
