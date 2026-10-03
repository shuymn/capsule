# Configuration

Read this file when editing configuration or adding prompt values. Use generic modules for tool-specific display; keep detectors in configuration.

## File and examples

Use an exported `XDG_CONFIG_HOME` plus `capsule/config.toml` when set. Relative values resolve from cwd; an empty value selects `capsule/config.toml` in cwd. Otherwise, lookup uses `~/.config/capsule/config.toml`, then `~/.capsule/config.toml`. A missing file uses defaults.

Keep one top-level `schema_version = 2`. `capsule preset` prints the complete [examples/config.toml](../examples/config.toml); select the needed modules when creating or merging configuration.

## Modules and values

Define modules with `[[module]]` and named candidate arrays under `[module.values]`. Module fields are `name`, `when`, `format`, `icon`, `style`, `connector`, `arbitration`, and `slot`. Each candidate sets exactly one source:

| Source | Behavior |
| --- | --- |
| `env = "NAME"` | Read the shell's exported value; preserve whitespace and distinguish empty from unset |
| `file = "relative/path"` | Read text relative to cwd; reject absolute paths and `.` / `..` components; trim whitespace |
| `command = ["program", "arg"]` | Execute argv directly with cwd and the full exported environment; trim stdout |

Try candidates in array order. Missing data, regex mismatch, command failure, and timeout permit fallback. The first success stops fallback, including an empty value. Optional `regex` extracts capture group 1. Shell syntax requires an explicit shell executable in argv. Consult [architecture](architecture.md) for resource limits.

Keep candidates semantically equivalent. A version file declares the requested version; a version command reports the executable selected by the shell. Use separate named values when displaying both.

Use `when.files` for regular-file presence and `when.env` for exported-variable presence. Each list uses OR; the two lists combine with AND. Empty lists impose no condition, and an empty exported value counts as present.

| Format | Behavior |
| --- | --- |
| `{name}` | Hide the module while this required value is unavailable |
| `[text {name}]` | Omit this section while its required value is unavailable; nesting is supported |
| `{{`, `[[` | Render literal `{`, `[` |

Keep declaration order. The default `slot = "line1"` places modules after Git; `"line2"` places them before time. `arbitration = { group = "runtime", priority = 10 }` chooses one ready module per group across both slots, preferring lower priority then declaration order.

## Display settings

| Section | Settings |
| --- | --- |
| `character` | `disabled`, `glyph`, `success_style`, `error_style`, `vicmd.glyph`, `vicmd.style` |
| `directory` | `disabled`, `style`, `read_only_style` |
| `git` | `disabled`, `icon`, `connector`, `style`, `indicator_style`, `state_style`, `detached_hash_style` |
| `cmd_duration` | `disabled`, `threshold_ms`, `connector`, `style` |
| `time` | `disabled`, `format` (`HH:MM:SS` / `HH:MM`), `connector`, `style` |
| `connectors` | `style` |
| `color_map` | symbolic color names mapped to 30–37 or 90–97 |

Styles accept `fg`, `bold`, and `dimmed`; omitted fields inherit their defaults. Colors are `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, and `bright_black`. A `vicmd.style` override applies independently of exit status. Read [style.rs](../crates/core/src/plan/style.rs) for defaults.

## Reload and verification

After a command or cwd change, the worker reads configuration and the exported environment. Width/keymap changes and empty Enter reuse observations. Invalid reloads report a diagnostic and retain the last valid plan; invalid startup configuration uses defaults.

Validate generated TOML with `ConfigPlan::parse`. Unknown fields, duplicate names, invalid sources/regex/formats, undefined values, and exceeded limits reject the document. Verify changed behavior in an isolated zsh home/config, including fallback order, empty versus unset values, optional sections, or arbitration as applicable. Use [migration.md](migration.md) for daemon-based installations.
