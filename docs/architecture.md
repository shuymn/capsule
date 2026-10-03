# Architecture

Read this file when changing session ownership, acquisition, rendering, or shell integration. Keep zsh input responsive on macOS and Linux and minimize the code one maintainer must own. Support the two-line prompt, directory, Git, status/vi mode, duration, optional time, configured values, styles, and reload.

## Ownership

Run one worker per interactive zsh session on Tokio's current-thread runtime. The public CLI is `init zsh` and `preset`; `worker` and `fd-config` are internal commands.

| Owner | Responsibility |
| --- | --- |
| [init.zsh](../crates/core/src/init/init.zsh) | Shell generation, full exported snapshot, nonblocking pipe buffers, fallback, zle hooks |
| [worker.rs](../crates/cli/src/worker.rs) | One active acquisition generation, newest pending replacement, reload, fresh observations, settled display snapshot, response writer |
| [plan.rs](../crates/core/src/plan.rs) | Immutable schema-v2 ConfigPlan: validated sources, conditions, indexed formats, display settings |
| [acquire.rs](../crates/core/src/acquire.rs), [plan/acquire.rs](../crates/core/src/plan/acquire.rs), [git.rs](../crates/core/src/git.rs) | Bounded env/file/argv acquisition, sequential fallback, Git CLI and directory facts |
| [view.rs](../crates/core/src/view.rs) | Pure evaluation, arbitration, structured text/style, grapheme layout, final zsh serialization |
| [session.rs](../crates/protocol/src/session.rs) | Authoritative byte-frame grammar and persistent cancellation-safe framing |

Use generic configuration for tool-specific values; see [extending.md](extending.md). Presets embed [examples/config.toml](../examples/config.toml). Keep acquisition I/O outside View.

## Behavioral contract

- WHILE acquisition is pending, Capsule SHALL accept input and preserve the typed buffer/cursor during redraw.
- WHEN a command executes or cwd changes, Capsule SHALL start a generation and reload configuration asynchronously. Capture cwd and the full exported environment as OS bytes after preserved `precmd_functions` hooks complete or abort, then send the request; retain status/duration capture in the first Capsule hook.
- WHILE revalidating an identical cwd/environment snapshot, Capsule SHALL retain the last settled external display, including directory metadata, Git, and custom modules; new partial results SHALL remain hidden.
- WHEN cwd or any exported variable's presence or bytes change, Capsule SHALL discard retained external display immediately. Environment ordering SHALL NOT affect snapshot identity.
- WHEN reload accepts different configuration source text (including removal), Capsule SHALL discard retained display before using the new plan. Until reload settles, same-input retention SHALL be provisional; a rejected configuration SHALL retain the last valid plan.
- WHEN a generation settles, including failure or timeout, Capsule SHALL atomically replace the external display with that generation's observations, removing results whose conditions are false or whose required values are Missing or Failed.
- WHEN exit status, duration, width, or keymap changes, Capsule SHALL render these local inputs without waiting for external acquisition. Width, keymap, and empty Enter SHALL NOT trigger reacquisition.
- WHEN transport fails or the worker stops, Capsule SHALL select fallback and discard worker-owned retained display; the next command SHALL start a replacement worker.
- WHEN spawning a command, Capsule SHALL use direct argv and replace its environment with the snapshot, preserving absent versus empty variables and non-UTF-8 bytes. Non-exported parameters, aliases, and functions are excluded.
- WHEN a snapshot exceeds its bound, Capsule SHALL omit acquisition rather than truncate the snapshot. Diagnostics SHALL exclude environment contents.
- WHEN configuration validation fails, Capsule SHALL reject the entire new plan, report the reason, and retain the last valid plan; a fresh worker uses defaults.
- WHEN evaluating a display snapshot, Capsule SHALL hide a module whose condition is pending, false, or failed. WHEN a required value is unavailable, Capsule SHALL hide the module; unavailable optional sections SHALL be omitted.
- WHEN a candidate is missing, fails extraction, fails execution, or times out, Capsule SHALL try the next candidate. WHEN a candidate is ready, including an empty value, Capsule SHALL stop fallback. Generation cancellation SHALL end acquisition.
- WHEN observations arrive in any order, Capsule SHALL retain declaration order and select at most one ready module per arbitration group across both slots, using lowest priority then declaration order.
- WHEN rendering data, Capsule SHALL display controls as data, keep text/style separate through grapheme-safe width adjustment, and serialize styles and zsh prompt escapes last.
- WHEN preserved `precmd_functions` hooks complete or abort, Capsule SHALL finalize its own retained prompt for the actual `PROMPT_SUBST` option before prompt expansion, keeping response text literal even without a new response. Capture the option and refresh retained text in the first Capsule hook as well. Run preserved hooks in order through `_capsule_run_precmd_hooks`, with snapshot capture, generation submission, and prompt finalization in its `always` block; preserve shell errors and skip subsequent user hooks after a genuine shell error, but continue after ordinary nonzero returns as zsh does. Give every preserved hook the original command exit status. Forward `add-zsh-hook` registration, removal, and listing to the captured hooks; preserve them across repeated initialization. Capsule SHALL preserve the user's option and leave user-owned prompts unchanged across fallback selection and synchronous/asynchronous responses; matching responses SHALL still replace Capsule's fallback.
- WHEN a response's generation differs from the current shell generation, Capsule SHALL leave the prompt unchanged.
- WHEN an accepted response leaves the displayed prompt unchanged, Capsule SHALL skip redraw; a matching result SHALL still replace fallback.
- WHEN I/O is partial or backpressured, Capsule SHALL retain framing state and complete the partial frame before replacement, or reset transport. Unterminated EOF frames SHALL be rejected; read credits SHALL resume pending shell writes.

Keep acquisition states distinct: `Pending` is unfinished, `Ready` contains a value, `Missing` has no matching source data, and `Failed` retains the acquisition error. Allocate fresh observations per generation; never copy retained display into acquisition. Decode OS bytes only at the display boundary. Trim file/command output, preserve environment text, and extract regex capture group 1.

Keep at most one settled display snapshot in the worker, not a cwd cache. Superseding unfinished work may retain that same snapshot only while inputs and the accepted plan remain unchanged. With no compatible settled display, render only local inputs until completion. The completion bit describes acquisition, not whether the displayed information is retained. This replaces unconditional hiding at every generation boundary: previously validated same-input display may remain visible, but unfinished new results may not.

## Resources and shutdown

| Boundary | Limit |
| --- | --- |
| Escaped request / response frame, excluding LF | 256 KiB / 64 KiB |
| Config file / one source value | 64 KiB / 64 KiB |
| Concurrent subprocesses / blocking file jobs | 4 / 2 per worker |
| One I/O operation, including capacity wait | 500 ms |
| Acquisition generation / process cleanup grace | 2 s / 100 ms |
| Modules / total values / total candidates | 64 / 256 / 1024 |
| Values per module / candidates per value | 16 / 8 |
| Format source / optional nesting depth | 4 KiB / 8 |
| Compiled regex size / nesting | 64 KiB / 32 |

Keep permit ownership with blocking tasks and delayed reapers until they finish. New generations must not accumulate replacement work behind abandoned operations.

Each command owns a process group. Clean up the group and reap the direct child after completion, cancellation, timeout, output overflow, or worker shutdown. Bound pipe cleanup when descendants retain stdout. Commands that deliberately leave the group/session are outside this lifecycle guarantee.

Use rustix for safe OS operations and CLOEXEC shell descriptors. Shell exit/exec closes the transport; worker EOF/signals cancel active work. Preserve these boundaries with the real-platform checks in [testing.md](testing.md). Use [migration.md](migration.md) for installed-service transitions.
