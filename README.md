# capsule

`capsule` is a macOS-only prompt engine for `zsh`, implemented in Rust.

The project ships a single `capsule` binary with three subcommands:

- `capsule daemon` starts the prompt daemon over a Unix domain socket
- `capsule connect` relays shell I/O to the daemon through a coprocess
- `capsule init zsh` prints the shell integration script

`zsh` stays thin, while the daemon handles prompt rendering, layout, caching, and slow module refreshes.

## Status

This repository is no longer a template. It currently implements:

- macOS + `zsh` support
- a two-line prompt
- daemon-backed rendering over `$TMPDIR/capsule.sock`
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
- detected toolchain (`rust`, `node`, `python`, `go`, `bun`, `ruby`, `elixir`)
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

To install the binary into Cargo's bin directory:

```bash
cargo install --path crates/cli --locked
```

## zsh Setup

Add this to `.zshrc`:

```zsh
eval "$(capsule init zsh)"
```

What the generated script does:

- creates a random session ID per shell session
- starts `capsule connect` as a coprocess relay
- sends prompt render requests from `precmd`
- tracks command duration from `preexec`
- falls back to `%~ %# ` if the coprocess is unavailable

The daemon is expected to use `$TMPDIR/capsule.sock`. When `CAPSULE_LOG` is set, daemon logs are written to `$TMPDIR/capsule.log`.

## CLI

```bash
capsule --help
capsule daemon
capsule connect
capsule init zsh
```

`capsule init zsh` is the only shell integration exposed today.

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
- `docs/architecture.md`: architecture baseline and constraints
- `docs/tooling.md`: build, lint, hook, and CI policy
