# Prompt Benchmarking

Use the prompt benchmark harness when you need a fair comparison between
`capsule` and `starship`.

## Design

- Use a **release** `capsule` binary for measurements (default `--capsule-bin`
  is `target/release/capsule`); debug builds are not representative for latency
  comparisons
- `capsule`: sends requests directly to the daemon Unix socket, measures
  RenderResult (fast) and Update (slow) latencies separately
- The daemon **may omit** `Update` when slow modules leave the composed prompt
  unchanged (common for the non-repository workload). The harness therefore uses a
  **short** socket read timeout for the optional second line only (see
  `UPDATE_WAIT_MS` in `capsule-prompt-bench`), so those cases do not stall for
  seconds per iteration; keep that value above your expected slow-path latency if
  you change workloads
- `starship`: runs `starship prompt` as a subprocess
- **Uncached** iterations use a unique subdirectory to prevent daemon cache hits,
  ensuring slow modules (git, toolchain) run every time
- **Cached** iterations reuse the same directory after warm-up so that the daemon
  serves results from its internal cache

## Workloads

The harness generates deterministic local workloads:

- non-repository directory
- Git repository with `Cargo.toml` and a `rustc --version` custom module

## Metrics

- **Fast (p50/p95)**: time until RenderResult arrives (directory, duration, time modules)
- **Slow (p50/p95)**: time until Update arrives (git status, toolchain commands)
- Starship reports only Fast (it computes everything synchronously)
- Statistics: `min`, `p50`, `p95`, `max`, `mean`, `stddev`

## Usage

Build the release binary first:

```bash
task build:release
```

Run the benchmark:

```bash
task bench:prompt
```

Direct invocation:

```bash
cargo run -p capsule-prompt-bench --bin prompt-bench --release -- --iterations 30 --json-out target/prompt-bench.json
```

## Reporting Constraints

- Treat the output as a standard-configuration comparison
- Do not describe the result as a pure renderer benchmark
- Do not delete outliers unless the exclusion rule was fixed before the run
