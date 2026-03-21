# capsule

`capsule` is a macOS-only prompt engine for `zsh`, implemented in Rust.

The project ships a single `capsule` binary:

- `capsule daemon` starts the prompt daemon (auto-detects launchd socket activation)
- `capsule daemon install` / `uninstall` manages the launchd service
- `capsule connect` relays shell I/O to the daemon through a coprocess
- `capsule init zsh` prints the shell integration script

`zsh` stays thin, while the daemon handles prompt rendering, layout, caching, and slow module refreshes.

## Status

This repository is no longer a template. It currently implements:

- macOS + `zsh` support
- a two-line prompt
- daemon-backed rendering over `~/.capsule/capsule.sock`
- launchd socket activation with `capsule daemon install`
- fast and slow prompt modules
- async refresh for slow prompt data after the initial render

Current limitations:

- `zsh` is the only supported shell
- macOS is the primary target platform
- prompt composition is fixed in code; there is no user config format yet

## Prompt Contents

The rendered prompt is split into two lines.

Line 1 currently combines available segments such as:

- current directory
- detected toolchain with version (`rust`, `node`, `python`, `go`, `bun`, `ruby`)
- Git branch and working tree indicators
- command duration when the previous command took at least 2 seconds

Line 2 currently shows:

- local time prefixed with `at`
- the prompt character `❯` (green on success, red on failure)

When the first line would overflow the terminal width, the directory segment is truncated first and trailing segments are dropped after that.

## Installation

Requirements:

- macOS
- `zsh`
- Rust toolchain from [rustup](https://www.rust-lang.org/tools/install)
- the repository pins `nightly` in `rust-toolchain.toml`
- optional: [Task](https://taskfile.dev/installation/) and [Lefthook](https://github.com/evilmartians/lefthook)

Build locally:

```bash
task build
```

Or run directly during development:

```bash
cargo run -p capsule-cli -- --help
```


## Setup

### 1. Install the binary

```bash
cargo install --path crates/cli --locked
```

### 2. Register with launchd (recommended)

```bash
capsule daemon install
```

This writes a plist to `~/Library/LaunchAgents/` and loads the service. launchd creates `~/.capsule/capsule.sock` and launches the daemon on first connection.

To remove:

```bash
capsule daemon uninstall
```

Without launchd, the daemon starts automatically when a shell opens (standalone mode).

### 3. Add to `.zshrc`

```zsh
eval "$(capsule init zsh)"
```

The generated script:

- creates a random session ID per shell session
- starts `capsule connect` as a coprocess relay
- sends prompt render requests from `precmd`
- tracks command duration from `preexec`
- falls back to `%~ %# ` if the coprocess is unavailable

When `CAPSULE_LOG` is set, daemon logs are written to `$TMPDIR/capsule.log`.

## CLI

```
capsule daemon              Start the daemon (auto-detects launchd)
capsule daemon install      Register launchd service
capsule daemon uninstall    Remove launchd service
capsule connect             Coprocess relay (used by init script)
capsule init zsh            Print shell integration script
```

## Development

Use Task as the default developer interface:

```bash
task
task build
task test
task lint
task fmt
task check
```

Benchmark prompt latency against `starship` in isolated `zsh` sessions:

```bash
task bench:prompt
```

Cargo equivalents:

```bash
cargo build --workspace --locked
cargo test --workspace --all-targets --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

To enable git hooks after installing Lefthook:

```bash
lefthook install
```

## Repository Layout

- `crates/cli`: CLI entrypoint and integration tests
- `crates/core`: daemon, init script generation, prompt modules, rendering
- `crates/protocol`: wire protocol, message codec, netstring framing
- `crates/sys`: platform-specific FFI (launchd socket activation)
- `docs/architecture.md`: architecture baseline and constraints
- `docs/benchmarking.md`: prompt benchmark rules and usage
- `docs/tooling.md`: build, lint, hook, and CI policy
