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
build. The script prints its artifact directory containing `results.json` and each
shell's `terminal.log`. Use `--output <new-directory>` to choose a fresh destination.

Require successful exit and all contract records:

| Scope | Required behavior |
|---|---|
| One shell | One worker; empty Enter preserves displayed information; resize/keymap changes reuse acquisition |
| Snapshot | Exported environment bytes and cwd reach acquisition; unset and unexported variables stay absent |
| Input | Delayed prompt updates preserve the typed buffer and cursor |
| Transport | A backpressured large request completes without further keyboard input; stale/future responses cannot replace the prompt |
| Recovery | Worker failure selects fallback; the next command starts a replacement worker |
| Exit and exec | Pipes close; worker and active acquisition descendants terminate |
| Ten shells | Ten distinct workers, one initial acquisition each, and no workers left active after exit |

Record each platform separately. A filesystem that rejects non-UTF-8 names produces
`non_utf8_fixture_supported = false`; require the Linux record with `true` for that
cwd boundary. Partial `results.json` output from a failed run is not a pass.

Also run `task test` on each platform for fragmented/cancelled frames, partial shell
reads, frame bounds, command/file limits, and process cleanup regressions. Suite
results and acquisition timings do not replace interactive PTY evidence. Follow
[benchmarking.md](benchmarking.md) for timing definitions and host requirements.
