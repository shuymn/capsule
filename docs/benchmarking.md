# Prompt Benchmarking

Use the prompt benchmark harness when you need a fair comparison between
`capsule` and `starship`.

## Design

- `capsule`: sends requests directly to the daemon Unix socket, measures
  RenderResult (fast) and Update (slow) latencies separately
- `starship`: runs `starship prompt` as a subprocess
- Each iteration uses a unique subdirectory to prevent daemon cache hits,
  ensuring slow modules (git, toolchain) run every time

## Workloads

The harness generates deterministic local workloads:

- non-repository directory
- small clean Git repository (24 files)
- medium clean Git repository (240 files)
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
uv run python tools/prompt_bench.py --iterations 30 --json-out target/prompt-bench.json
```

## Reporting Constraints

- Treat the output as a standard-configuration comparison
- Do not describe the result as a pure renderer benchmark
- Do not delete outliers unless the exclusion rule was fixed before the run
