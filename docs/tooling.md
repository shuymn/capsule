# Tooling Pipeline

Read this file when working with build, CI, hooks, or adding tools.

## Source of Truth

- Use `task` as the primary interface for local development, CI, and git hooks.
- [Taskfile.yml](../Taskfile.yml) is the source of truth for task definitions. Build output lives under `target/` (Cargo default).

## Hooks and CI

- [lefthook.yml](../lefthook.yml) maps git hook events to `task` commands. Do not duplicate hook logic in shell scripts.
- Hooks run in `piped` mode, can auto-stage formatter fixes, and skip merge or rebase flows.
- CI mirrors the same `task` commands used locally (`fmt:check`, `lint`, `test`, `build`).
- Rust on CI uses [actions-rust-lang/setup-rust-toolchain](https://github.com/actions-rust-lang/setup-rust-toolchain) pinned to a full commit SHA (under the `rust-lang` GitHub org). Caching is disabled so the workflow does not pull in an extra third-party cache action; toolchain install still uses the official `https://sh.rustup.rs` script inside that composite action.
- That action sets **`RUSTFLAGS=-D warnings`** by default for the job, so `task test` and `task build` in CI fail on **rustc** warnings too (not only Clippy). Locally, important lints are already denied via `Cargo.toml` `[lints]`; remaining rustc warnings are caught by `task lint` and CI.
- The default toolchain is **nightly** ([rust-toolchain.toml](../rust-toolchain.toml)) so `cargo fmt` respects unstable options in [rustfmt.toml](../rustfmt.toml). For a quieter baseline, pin nightly to a specific date in `rust-toolchain.toml`.
- **Clippy policy** (this is the canonical reference; other docs defer here):
  - [Cargo.toml](../Cargo.toml) `[lints.rust]` / `[lints.clippy]`: denies common footguns (`unwrap_used`, `expect_used`, `todo`, `dbg_macro`, etc.). These apply to all code including tests.
  - [clippy.toml](../clippy.toml): tightens complexity and size thresholds.
  - Crate root (`main.rs` or `lib.rs`): `#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]` — these become errors under `task lint` (`-D warnings`). When adding a new crate root, copy these attributes.

## Adding Tools

1. Prefer Cargo-installed tools or rustup components when possible.
2. If you add a new binary dependency, document it in the README and wire optional automation through Task (`preconditions` / `status`) instead of ad-hoc scripts.
