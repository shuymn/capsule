# Tooling

Read this file when changing build, CI, hooks, or tool dependencies.

## Commands and configuration

- Use the tasks in [mise.toml](../mise.toml) as the command source of truth; list them
  with `mise tasks` (`mise tasks --json` for machines). Run `mise trust` and
  `mise install` once per checkout. Use the stable toolchain in
  [rust-toolchain.toml](../rust-toolchain.toml) and formatting settings in
  [rustfmt.toml](../rustfmt.toml).
- mise pins exact lefthook and uv versions in `[tools]`; Renovate updates them. Their
  aqua backend verifies release checksums and attestations at install time, so no
  `mise.lock` is committed (it would make Renovate bumps fail `--locked` installs).
- Install zsh, Git, and `ps` from the system, not mise: macOS ships them, and Linux
  uses the distribution package (`apt-get install zsh`). Rust stays on rustup;
  only fuzzing needs a nightly toolchain (`rustup toolchain install nightly`).
- `mise run check` and `mise run check:fast` install lefthook hooks. To preserve
  existing hooks, run `mise run verify` (the same steps as `check`) or individual
  tasks such as `mise run lint`.
- [lefthook.yml](../lefthook.yml) owns hook behavior, including formatter auto-staging
  and merge/rebase skips. Keep hook logic in mise tasks rather than duplicating scripts.
- [CI](../.github/workflows/ci.yml) sets up tools with jdx/mise-action and uses the
  same mise tasks on macOS and Linux. Each workflow pins mise in `CI_MISE_VERSION`
  (a Renovate regex manager updates it); the helper image copies mise from a
  digest-pinned `ghcr.io/jdx/mise` image. `min_version` in mise.toml is a contributor
  floor; raise it by hand only when mise.toml needs a newer mise. The Rust setup action treats rustc warnings as errors through
  `RUSTFLAGS=-D warnings`.
- Run Python scripts through uv with the version in [.python-version](../.python-version).
- Use `mise run docker:linux:lint` or `mise run docker:linux:check` for Linux checks
  from another OS. They build [the helper image](../docker/linux-ci.Dockerfile), which
  preinstalls the mise tools, mount the repository, and run with the host
  UID/GID. The check task skips hook installation.
- Follow [testing.md](testing.md) for suite and PTY checks,
  [benchmarking.md](benchmarking.md) for latency comparisons and acquisition diagnostics, and
  [releasing.md](releasing.md) for version, candidate, and promotion rules.

## Clippy policy

- [Cargo.toml](../Cargo.toml) defines workspace lint levels, including forbidden
  unsafe code and denied `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`dbg!` use in all code and tests.
- [clippy.toml](../clippy.toml) defines complexity/size thresholds. Enable
  `#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]` in each crate root;
  `mise run lint` promotes these warnings to errors.
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

Prefer Cargo tools or rustup components; otherwise pin an exact version in mise.toml
`[tools]`. Pin a tool used only by an opt-in task in that task's `tools`, with a
Renovate regex manager, so `mise install` stays minimal. Document new binary dependencies in the README and guard optional
automation with checks inside the mise task, as `install:lefthook` does.
