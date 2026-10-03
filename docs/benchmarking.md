# Prompt Acquisition Diagnostics

Use `capsule-prompt-bench` to verify acquisition work and report its duration. Use
[testing.md](testing.md#interactive-shell-contract) for interactive shell behavior.

## Measurement boundary

| Path | `fast` | `slow` |
|---|---|---|
| Capsule persistent worker | Request start to first matching response | Same start to matching `complete = 1` response |
| Starship subprocess | Process startup through synchronous prompt acquisition | Not reported |

Capsule may coalesce renders; an already-complete first response gives equal `fast`
and `slow` durations. Neither path measures interactive zsh startup, input, or
visible redraw latency. Report paths independently; their boundaries do not support a
cross-tool speedup claim.

Both tools use generated configuration and an isolated HOME. The rustc logging
wrapper and subprocess polling add diagnostic overhead. These results describe
the fixtures, not tool defaults. Use busy machines for correctness checks only;
their timings must not determine library or architecture adoption. Performance
evidence requires an isolated host and recorded hardware, load, build profile,
sample count, configuration, and measurement boundary.

## Workload and completion

The harness creates a non-repository workload and a Git repository with `Cargo.toml`
in every measured toolchain directory. Both tools explicitly run `rustc --version`.
Capsule receives a schema-v2 configuration and the complete isolated environment.

Require these checks for every warm-up and measured sample:

- New Capsule generation or Starship invocation: exactly one successful rustc start
  and completion with rendered version output for the toolchain workload; zero calls otherwise.
- Capsule same-generation redraw: reuse completed acquisition and require zero new
  rustc calls. Alternate width/keymap and match the expected character to reject
  buffered duplicate responses. This phase measures worker rerendering, not reacquisition.
- Worker response: skip `K` credits, match the exact generation, and require
  `complete = 1` after acquisition and cleanup. Reject invalid frames, EOF, missing
  completion/output, failed calls, unexpected counts, or deadline expiry.
- Subprocess/shutdown: require successful exit, bounded waits, and worker stdin
  closure with stdout drained before reaping.

Report verified rustc calls per row, excluding warm-up. Keep all samples unless an
exclusion rule was fixed before the run.

## Commands

Run fixture, count, completion, and deadline checks with
`cargo test -p capsule-prompt-bench --locked`.

For a diagnostic run, use `task bench:prompt`; it builds release
artifacts and requires Starship, Git, and rustc on PATH. To select binaries, sample
count, or report files, run the harness directly after `task build:release`:

```sh
cargo run -p capsule-prompt-bench --bin prompt-bench --release --locked -- \
  --capsule-bin target/release/capsule --starship-bin starship --git-bin git \
  --iterations 100 --json-out /tmp/capsule-bench.json --markdown-out /tmp/capsule-bench.md
```
