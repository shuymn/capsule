# capsule

`capsule` is a prompt engine for `zsh`, implemented in Rust. Runs on macOS and Linux.

A persistent daemon handles rendering, caching, and slow module refreshes. `zsh` relays prompt requests through a coprocess, so the prompt renders immediately and updates asynchronously when background work completes.

## Prompt

```
<directory> on <git branch> [indicators] via <toolchain> took <duration>
at <time> ❯
```

**Line 1:** directory, git status, custom modules, command duration. Toolchain segments (the `via <toolchain>` part) have no built-in implementation — they are provided entirely by user-defined `[[module]]` entries.

**Line 2:** time (disabled by default), prompt character `❯` / `❮` (vim command mode). Character is green on success, red on failure.

Line 1 truncates the directory first and drops trailing segments when it would overflow the terminal width.

## Installation

Requirements: macOS or Linux, `zsh`, Rust `nightly` (pinned in `rust-toolchain.toml`).

```bash
# 1. Install the binary
cargo install --git https://github.com/shuymn/capsule --package capsule-cli --locked

# 2. Register with the system service manager (recommended)
capsule daemon install   # macOS: launchd  |  Linux: systemd --user

# 3. Add to .zshrc
eval "$(capsule init zsh)"
```

To bootstrap toolchain modules, run `capsule preset` and paste the output into your config file.

## Configuration

Config file is loaded from the first path that exists:

1. `$XDG_CONFIG_HOME/capsule/config.toml`
2. `~/.config/capsule/config.toml`
3. `~/.capsule/config.toml`

Changes are hot-reloaded on the next prompt render.

### Built-in modules

```toml
[character]
glyph = "❯"
success_style = { fg = "green", bold = true }
error_style = { fg = "red", bold = true }

[character.vicmd]           # vim command mode override
glyph = "❮"
# style = { fg = "yellow" }

[directory]
style = { fg = "cyan", bold = true }
# read_only_style = { fg = "red" }

[git]
icon = "\u{f418}"
connector = "on"
style = { fg = "magenta", bold = true }
indicator_style = { fg = "red", bold = true }
# detached_hash_style = { fg = "green", bold = true }
# state_style = { fg = "yellow", bold = true }

[time]
disabled = true             # set to false to enable
format = "HH:MM:SS"         # or "HH:MM"
connector = "at"
style = { fg = "yellow", bold = true }

[cmd_duration]
threshold_ms = 2000
connector = "took"
style = { fg = "yellow", bold = true }
```

### Connectors and timeouts

```toml
[connectors]
# style = {}

[timeout]
fast_ms = 500       # env/file sources
slow_ms = 5000      # commands, git
```

### Style syntax

| Key      | Type  | Description         |
|----------|-------|---------------------|
| `fg`     | color | Foreground color    |
| `bold`   | bool  | Bold text           |
| `dimmed` | bool  | Dimmed (faint) text |

Colors: `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `bright_black`.

Override ANSI codes with `[color_map]` (classic 30–37 and bright 90–97):

```toml
[color_map]
green = 32
cyan = 36
```

### Custom modules

```toml
[[module]]
name = "rust"
when.files = ["Cargo.toml"]
format = "v{version}"
icon = "🦀"
connector = "via"
style = { fg = "red" }

# Sources with the same `name` form a fallback chain; first match wins.
[[module.source]]
name = "version"
env = "RUST_VERSION"

[[module.source]]
name = "version"
command = ["rustc", "--version"]
regex = 'rustc ([\d.]+)'
```

Env/file sources are evaluated inline. Command sources run in the background and update the prompt asynchronously.

#### Format string syntax

| Syntax   | Meaning |
|----------|---------|
| `{name}` | Variable placeholder; module suppressed if unresolved |
| `[…]`    | Optional section; omitted if any variable inside is unresolved |
| `{{`     | Literal `{` |
| `[[`     | Literal `[` |

```toml
format = "{profile}[ ({region})]"   # region omitted when unresolved
```

#### Arbitration

When multiple modules can fire in the same directory, only the lowest-`priority` module in a group renders:

```toml
arbitration = { group = "runtime", priority = 10 }
```

Modules without `arbitration` always render.

## CLI

```
capsule daemon              Start the daemon
capsule daemon install      Register service (launchd on macOS, systemd on Linux)
capsule daemon uninstall    Remove service
capsule connect             Coprocess relay (used by init script)
capsule init zsh            Print shell integration script
capsule preset              Print built-in module definitions as TOML
```

## Repository Layout

- `crates/cli`: CLI entrypoint and integration tests
- `crates/core`: daemon, prompt modules, rendering, configuration
- `crates/prompt-bench`: benchmark harness
- `crates/protocol`: wire protocol and message codec
- `crates/sys`: platform-specific FFI (launchd on macOS, systemd socket activation on Linux)
