# Architecture

Read this file when changing session ownership, acquisition, rendering, or shell integration. Keep zsh input responsive on macOS and Linux and minimize the code one maintainer must own. Support the two-line prompt, directory, Git, status/vi mode, duration, optional time, configured values, styles, and reload.

## Ownership

Run one worker per interactive zsh session on Tokio's current-thread runtime. The public CLI is `init zsh` and `preset`; `worker` and `fd-config` are internal commands.

| Owner | Responsibility |
| --- | --- |
| [init.zsh](../crates/core/src/init/init.zsh) | Shell generation, full exported snapshot, nonblocking pipe buffers, fallback, zle hooks |
| [worker.rs](../crates/cli/src/worker.rs) | One active acquisition generation, newest pending replacement, reload, observations, response writer |
| [plan.rs](../crates/core/src/plan.rs) | Immutable schema-v2 ConfigPlan: validated sources, conditions, indexed formats, display settings |
| [acquire.rs](../crates/core/src/acquire.rs), [plan/acquire.rs](../crates/core/src/plan/acquire.rs), [git.rs](../crates/core/src/git.rs) | Bounded env/file/argv acquisition, sequential fallback, Git CLI and directory facts |
| [view.rs](../crates/core/src/view.rs) | Pure evaluation, arbitration, structured text/style, grapheme layout, final zsh serialization |
| [session.rs](../crates/protocol/src/session.rs) | Authoritative byte-frame grammar and persistent cancellation-safe framing |

Use generic configuration for tool-specific values; see [extending.md](extending.md). Presets embed [examples/config.toml](../examples/config.toml). Keep acquisition I/O outside View.

## Behavioral contract

- WHEN acquisition is pending, Capsule SHALL accept input using local information or fallback and preserve the typed buffer/cursor during redraw.
- WHEN a command executes or cwd changes, Capsule SHALL start a generation, reload configuration, capture cwd and the full exported environment as OS bytes, and hide unvalidated external results.
- WHEN only width, keymap, or empty Enter changes the prompt, Capsule SHALL reuse the generation's observations without reacquisition.
- WHEN spawning a command, Capsule SHALL use direct argv and replace its environment with the snapshot, preserving absent versus empty variables and non-UTF-8 bytes. Non-exported parameters, aliases, and functions are excluded.
- WHEN a snapshot exceeds its bound, Capsule SHALL omit acquisition rather than truncate the snapshot. Diagnostics SHALL exclude environment contents.
- WHEN configuration validation fails, Capsule SHALL reject the entire new plan, report the reason, and retain the last valid plan; a fresh worker uses defaults.
- WHEN a condition is pending, false, or failed, Capsule SHALL hide its module. WHEN a required value is unavailable, Capsule SHALL hide the module; unavailable optional sections SHALL be omitted.
- WHEN a candidate is missing, fails extraction, fails execution, or times out, Capsule SHALL try the next candidate. WHEN a candidate is ready, including an empty value, Capsule SHALL stop fallback. Generation cancellation SHALL end acquisition.
- WHEN observations arrive in any order, Capsule SHALL retain declaration order and select at most one ready module per arbitration group across both slots, using lowest priority then declaration order.
- WHEN rendering data, Capsule SHALL display controls as data, keep text/style separate through grapheme-safe width adjustment, and serialize styles and zsh prompt escapes last.
- WHEN a response's generation differs from the current shell generation, Capsule SHALL leave the prompt unchanged.
- WHEN an accepted response leaves the displayed prompt unchanged, Capsule SHALL skip redraw; a matching result SHALL still replace fallback.
- WHEN I/O is partial or backpressured, Capsule SHALL retain framing state and complete the partial frame before replacement, or reset transport. Unterminated EOF frames SHALL be rejected; read credits SHALL resume pending shell writes.

Keep acquisition states distinct: `Pending` is unfinished, `Ready` contains a value, `Missing` has no matching source data, and `Failed` retains the acquisition error. Decode OS bytes only at the display boundary. Trim file/command output, preserve environment text, and extract regex capture group 1.

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
