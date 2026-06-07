# Extending capsule

Read this file when adding prompt segments, team presets, or automating config changes (including AI agents).

## Extension surfaces

| Surface | Use for |
|---------|---------|
| `[[module]]` in config | Custom prompt segments (env / file / command sources) |
| `capsule preset` | Bootstrap toolchain module TOML |
| `[character]`, `[git]`, … | Built-in module styling only |

Rust `Module` implementations are **not** a public extension point (`sealed` trait). Extend via config DSL.

## Self-modification loop (agents)

1. **Bootstrap toolchains:** `capsule preset` and paste into user config.
2. **Verify:** `task test` and open a shell in the target directory to inspect the prompt.
3. **Env-dependent modules:** after adding new `env` sources, **reconnect** the coproc (`exec zsh` or new terminal). HelloAck publishes required env var names only at connect time.

Hot-reload picks up TOML edits on the next prompt without restarting the daemon.

## Module slots

```toml
[[module]]
name = "k8s"
slot = "line2"   # default: "line1"
```

- `line1` — after git, before command duration (default).
- `line2` — before time on the input line.

Line-1 segment order among built-ins is fixed; see [architecture.md](architecture.md).

## Further reading

- [architecture.md](architecture.md) — negative space, request pipeline stages
- [README.ja.md](../README.ja.md) — user-facing config examples
