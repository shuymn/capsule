# Prompt Benchmarking

Use the prompt benchmark harness when you need a fair, isolated comparison between
`capsule` and `starship`.

## Goals

- Compare standard configurations, not feature-parity custom configs
- Isolate the benchmark from the user's existing `zsh` dotfiles and plugins
- Measure shell-level prompt latency rather than internal render-function microbenchmarks
- Report cold start, warm steady-state, and context-change cases separately

## Isolation Rules

- Each sample creates a fresh temporary `HOME`, `ZDOTDIR`, and `TMPDIR`
- The harness launches `zsh -d -i` so global startup files are skipped
- `.zshenv` unsets `GLOBAL_RCS`, and `.zshrc` contains only the target prompt init plus the shared benchmark marker hook
- The environment starts from `env -i` semantics with only `HOME`, `ZDOTDIR`, `PATH`, `TERM`, `TMPDIR`, and `COLUMNS`
- `capsule` uses a per-session `TMPDIR`, so daemon sockets and logs stay isolated

## Workloads

The harness generates deterministic local workloads:

- non-repository directory
- small clean Git repository
- medium clean Git repository
- Git repository with `Cargo.toml` to trigger toolchain detection
- deep subdirectory inside the small Git repository

It also measures Git state-change prompts by creating an untracked file and by
modifying a tracked file inside the small repository.

## Metrics

- Primary metric: prompt stable time in milliseconds
- Reported statistics: `min`, `p50`, `p95`, `max`, `mean`, `stddev`
- `capsule` async updates are reported separately when observed within the async observation window

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
- Keep cold start, warm steady-state, and context-change results separate
- Do not delete outliers unless the exclusion rule was fixed before the run
