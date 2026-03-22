# capsule

`capsule` is a macOS-only prompt engine for `zsh`, implemented in Rust.

The project ships a single `capsule` binary:

- `capsule daemon` starts the prompt daemon (auto-detects launchd socket activation)
- `capsule daemon install` / `uninstall` manages the launchd service
- `capsule connect` relays shell I/O to the daemon through a coprocess
- `capsule init zsh` prints the shell integration script

`zsh` stays thin, while the daemon handles prompt rendering, layout, caching, and slow module refreshes.

## Prompt Contents

The rendered prompt is a two-line layout:

```
<directory> on <git branch> [indicators] via <toolchain> took <duration>
at <time> ❯
```

**Line 1** combines available segments:

- current directory (git-aware: repo-relative inside a git repo, home-abbreviated outside)
- Git branch and working tree indicators (`=` conflicted, `$` stashed, `✘` deleted, `»` renamed, `!` modified, `+` staged, `?` untracked, `⇡` ahead, `⇣` behind, `⇕` diverged)
- user-defined custom modules (e.g. toolchain versions detected by marker files or env vars)
- command duration when the previous command exceeded the threshold (default 2 s)

**Line 2** shows:

- local time (default `HH:MM:SS`)
- prompt character `❯` (green on success, red on failure)

When the first line would overflow the terminal width, the directory segment is truncated first and trailing segments are dropped after that.

## Installation

Requirements:

- macOS
- `zsh`
- Rust toolchain from [rustup](https://www.rust-lang.org/tools/install)
- the repository pins `nightly` in `rust-toolchain.toml`

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

- starts `capsule connect` as a coprocess relay
- sends prompt render requests from `precmd`
- tracks command duration from `preexec`
- receives async updates when slow modules complete
- falls back to `%~ %# ` if the coprocess is unavailable

When `CAPSULE_LOG` is set, daemon logs are written to `$TMPDIR/capsule.log`.

## Configuration

Configuration is loaded from `$XDG_CONFIG_HOME/capsule/config.toml` (fallback `~/.capsule/config.toml`). If the file is missing, compiled-in defaults are used. Changes are hot-reloaded on next prompt render without restarting the daemon.

### Built-in modules

```toml
[character]
glyph = "❯"
success_color = "green"
error_color = "red"
# success_style = { bold = true }
# error_style = {}

[directory]
color = "cyan"
style = { bold = true }
# read_only_style = { fg = "red" }

[git]
icon = "\u{f418}"                # Nerd Font glyph
indicator_color = "red"
style = { fg = "magenta", bold = true }
indicator_style = { bold = true }
# detached HEAD shows: HEAD (short_sha) — `HEAD ` uses `style`, `(short_sha)` uses:
# detached_hash_color = "green"
# detached_hash_style = { bold = true }

[time]
enabled = true
format = "HH:MM:SS"             # or "HH:MM"
color = "yellow"
style = { bold = true }

[cmd_duration]
threshold_ms = 2000
color = "yellow"
style = { bold = true }
```

### Connectors and timeouts

```toml
[connectors]
git = "on"                       # "on main"
time = "at"                      # "at 12:34:56"
cmd_duration = "took"            # "took 2.5s"
# style = {}

[timeout]
fast_ms = 500                    # env/file sources
slow_ms = 5000                   # commands, git
```

### Color map

Override the ANSI foreground codes behind each symbolic color name. Only classic (30–37) and bright (90–97) codes are accepted.

```toml
[color_map]
red = 31
green = 32
yellow = 33
blue = 34
magenta = 35
cyan = 36
bright_black = 90
```

### Style syntax

Each style field accepts an object with optional keys:

| Key      | Type    | Description          |
|----------|---------|----------------------|
| `fg`     | color   | Foreground color     |
| `bold`   | bool    | Bold text            |
| `dimmed` | bool    | Dimmed (faint) text  |

Available colors: `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `bright_black`.

### Custom modules

Define additional prompt segments via `[[module]]` entries. Custom modules appear on line 1 alongside the built-in segments.

```toml
[[module]]
name = "rust"
when.files = ["Cargo.toml"]          # trigger when these files exist in cwd
# when.env = ["RUST_VERSION"]        # or when env vars are set
format = "v{value}"                  # {value} is replaced by the resolved source
icon = "🦀"
color = "red"
connector = "via"                    # "via 🦀 v1.82.0"

# Sources are tried in order; the first match wins.
[[module.source]]
env = "RUST_VERSION"

[[module.source]]
command = ["rustc", "--version"]
regex = 'rustc ([\d.]+)'            # capture group 1 is the value
```

Sources that read env vars or files are fast (evaluated inline). Sources that run commands are slow (evaluated in the background; the prompt updates asynchronously when the result is ready).

#### Arbitration

When multiple custom modules could fire in the same directory, arbitration picks a single winner per group:

```toml
[[module]]
name = "node"
when.files = ["package.json"]
arbitration = { group = "runtime", priority = 10 }
# ...

[[module]]
name = "bun"
when.files = ["bun.lockb"]
arbitration = { group = "runtime", priority = 20 }
# ...
```

Lower `priority` wins. Modules without `arbitration` always render.

## CLI

```
capsule daemon              Start the daemon (auto-detects launchd)
capsule daemon install      Register launchd service
capsule daemon uninstall    Remove launchd service
capsule connect             Coprocess relay (used by init script)
capsule init zsh            Print shell integration script
```

## Benchmark

Prompt latency measured with `crates/prompt-bench`. *capsule* talks directly to the daemon socket; *starship* is invoked as a subprocess for comparison. Values show fast / slow where applicable (fast = `RenderResult`, slow = +`Update` with git/toolchain).

| Workload | Tool | p50 ms | p95 ms | vs starship |
| --- | --- | ---: | ---: | ---: |
| outside | capsule | 0.76 | 1.34 | x4.7 |
| outside | capsule (cached) | 0.13 | 0.87 | x27.9 |
| outside | starship | 3.55 | 4.10 | |
| repo-toolchain | capsule | 0.08 / 4.34 | 0.15 / 6.41 | x149.3 / x2.6 |
| repo-toolchain | capsule (cached) | 1.14 | 2.10 | x9.9 |
| repo-toolchain | starship | 11.24 | 12.24 | |

30 iterations per workload, release build, macOS (Apple Silicon). See `docs/benchmarking.md` for methodology and reproduction steps.

## Repository Layout

- `crates/cli`: CLI entrypoint and integration tests
- `crates/core`: daemon, init script generation, prompt modules, rendering, configuration
- `crates/prompt-bench`: benchmark harness comparing capsule and starship latency
- `crates/protocol`: wire protocol, message codec, netstring framing
- `crates/sys`: platform-specific FFI (launchd socket activation)
