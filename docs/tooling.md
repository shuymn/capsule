# Tooling

Read this file when changing build, CI, hooks, or tool dependencies.

## Commands and configuration

- Use [Taskfile.yml](../Taskfile.yml) as the command source of truth. Use the stable
  toolchain in [rust-toolchain.toml](../rust-toolchain.toml) and formatting settings
  in [rustfmt.toml](../rustfmt.toml).
- `task check` and `task check:fast` install lefthook hooks. To preserve existing
  hooks, run the needed components directly: `task fmt:check`, `task lint`,
  `task release:test`, `task test`, `task doc`, and `task build`.
- [lefthook.yml](../lefthook.yml) owns hook behavior, including formatter auto-staging
  and merge/rebase skips. Keep hook logic in Task rather than duplicating scripts.
- [CI](../.github/workflows/ci.yml) uses the same Task commands on macOS and Linux.
  The Rust setup action treats rustc warnings as errors through `RUSTFLAGS=-D warnings`.
- Use `task docker:linux:lint` or `task docker:linux:check` for Linux checks from
  another OS. They build [the helper image](../docker/linux-ci.Dockerfile), mount the
  repository, and run with the host UID/GID. The check task skips hook installation.
- Follow [testing.md](testing.md) for suite and PTY checks,
  [benchmarking.md](benchmarking.md) for acquisition diagnostics, and
  [releasing.md](releasing.md) for version, candidate, and promotion rules.

## Clippy policy

- [Cargo.toml](../Cargo.toml) defines workspace lint levels, including forbidden
  unsafe code and denied `unwrap`/`expect`/`todo`/`dbg!` use in all code and tests.
- [clippy.toml](../clippy.toml) defines complexity/size thresholds. Enable
  `#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]` in each crate root;
  `task lint` promotes these warnings to errors.
- Keep duplicate-crate checks enabled. The named exceptions cover incompatible
  proc-macro `syn` versions and WASI binding/parser `wit-bindgen`/`hashbrown` versions.
  After dependency updates, inspect both commands below; metadata may retain
  WASI-only macro dependencies absent from tree output. Remove an exception when
  its duplicate disappears, and retain diagnostics for the host macOS/Linux graph.

```sh
cargo tree --workspace --all-features --target all --locked -d
cargo metadata --all-features --locked
```

## Adding tools

Prefer Cargo tools or rustup components. Document new binary dependencies in the
README and wire optional automation through Task `preconditions`/`status` checks.
