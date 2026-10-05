# Testing

Read this file before writing or modifying tests.

## Suite and conventions

- Use `task test` for unit/integration targets and doctests. Cargo's `--all-targets`
  does not include doctests; the task runs them separately.
- Use `cargo test -p <crate> <filter> --locked` for focused checks. See
  [tooling.md](tooling.md) for full verification and Git-hook side effects.
- Put unit tests in a final `#[cfg(test)] mod tests`, inline or in `tests.rs`.
  Put public API and CLI integration tests under `tests/` and shared helpers in
  `tests/common/mod.rs`, which Cargo does not treat as a separate test binary.
- Use isolated fixtures, `Result` with `?` for fallible setup, and RAII cleanup.
  Assert behavior, including relevant failure/cancellation paths, without depending
  on execution order. Explain the reason and removal condition for any ignored test.
- Keep doctests runnable; use hidden `# ` setup, `no_run` for external resources,
  and `compile_fail` for invalid usage.

## Interactive shell contract

Run [scripts/session_pty.py](../scripts/session_pty.py) on both macOS and Linux when
changing shell integration, worker transport, or acquisition lifecycle. It requires
zsh, Python 3, and `ps`, uses real PTYs and disposable homes, and leaves user shell
configuration untouched. Build and run on the target OS from the repository root.

macOS and Linux:

```sh
task build
uv run --no-project scripts/session_pty.py
```

In Linux images without uv, run `python3 scripts/session_pty.py` after the build.

The default binary is `target/debug/capsule`; use `--binary <path>` for another
build, including a renamed executable. Require that exact build for `init`,
`worker`, and `fd-config`: the driver puts a disposable `capsule` symlink first on
each shell's PATH and probes a renamed target beside a conflicting `capsule`.
The script prints its artifact directory containing `results.json` and each shell's
`terminal.log`. Use `--output <new-directory>` to choose a fresh destination.

The flicker setup directly launches each fresh Git/tool executable once before
starting zsh, with a separate 8s timeout, EOF stdin, disposable `HOME`, cwd
`$HOME/work`, and fixture bin first on `PATH`. Require successful exit, exact
output, and done markers. Verify the setup-only tool count is exactly one, then
remove the count and both done markers before shell startup. Keep the production
500ms deadline, generation counts, accepted-completion witnesses, and gate
deadlines unchanged; do not retry failed scenarios.

Require successful exit and all contract records:

| Scope | Required behavior |
|---|---|
| Binary selection | A renamed selected executable supplies init, worker, and fd-config even beside a different executable named capsule |
| Cleanup safety | Confirmed-dead worker identities are retired; historical PIDs never become signal targets after reuse |
| One shell | One worker; empty Enter preserves displayed information; resize/keymap changes reuse acquisition |
| Initialization | With `add-zsh-hook` already loaded by another integration, ordinary commands advance generations and update real Git state and file-backed modules using the same worker |
| Revalidation | With file-controlled slow acquisition and unchanged cwd/environment/config, ordinary commands preserve Git/tool information in every intermediate frame and actual shell prompt assignment, including finalized precmd; the first command after a directory-aware precmd hook updates exports also preserves the settled display |
| Replacement | Changed results replace retained display at completion; Missing, Failed, and false conditions remove obsolete information |
| Invalidation | Changed cwd/exported environment immediately discard retained display; accepted changed configuration discards it after reload |
| Local display | Status, duration, resize, and keymap changes render while acquisition is held pending |
| Snapshot | Exported environment bytes and cwd reach acquisition after preserved user precmd hooks complete; directory-hook exports reach the directory-change generation; unset and unexported variables stay absent |
| Input | Delayed prompt updates preserve the typed buffer and cursor |
| Transport | A backpressured large request completes without further keyboard input; stale/future responses cannot replace the prompt |
| Recovery | Worker failure selects fallback; the next command starts a replacement worker even if a preserved precmd hook raises a genuine shell error |
| Exit and exec | Pipes close; worker and active acquisition descendants terminate; exec cleanup is checked after a replacement-ready marker and while the controlled replacement remains alive |
| Ten shells | Ten distinct workers, one initial acquisition each, and no workers left active after exit |

Record each platform separately. A filesystem that rejects non-UTF-8 names produces
`non_utf8_fixture_supported = false`; require the Linux record with `true` for that
cwd boundary. Partial `results.json` output from a failed run is not a pass.

Also run `task test` on each platform for fragmented/cancelled frames, partial shell
reads, frame bounds, command/file limits, and process cleanup regressions. Require
the shell regression with real initialization and installed precmd order to keep
retained dollar/backtick payloads literal when a preserved user hook toggles
`PROMPT_SUBST` in either direction, before any new response is released. Require
real interactive hook-error regressions for command-level and hook-level option
transitions, user-hook short-circuiting, status/duration capture, snapshot
invalidation, and replacement-worker startup. Require initialized shell regressions
to preserve user-owned prompts during changed snapshots, ready responses, and
worker startup. Verify hook ordering, each hook's original command status, and
continuation after ordinary nonzero returns, plus `add-zsh-hook` registration, removal, and listing after repeated
initialization with `add-zsh-hook` absent, marked for autoload, or already loaded.
Suite results and acquisition timings do not replace interactive PTY evidence. Follow
[benchmarking.md](benchmarking.md) for timing definitions and host requirements.
