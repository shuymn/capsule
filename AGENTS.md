<!-- Maintenance: Update when tasks, hooks, or project scope changes. -->
<!-- Audience: All docs under docs/ and this file are written for coding agents (LLMs), not humans. Use direct instructions, not tutorials or explanations of concepts the agent already knows. Apply this rule when creating or updating any documentation. -->

## Build, Test, and Development Commands

- Use Task ([Taskfile.yml](Taskfile.yml)) as the default interface
- `task build` / `task test` / `task lint` / `task fmt` / `task check` — primary workflow; `task check` runs formatting check, Clippy, tests, `cargo doc`, and build; `task check:fast` skips tests and docs (see [docs/tooling.md](docs/tooling.md))
- Rust-native equivalents work without Task: `cargo build`, `cargo test`, `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` (same as `task lint`; see [docs/tooling.md](docs/tooling.md) for Clippy policy details)
- Prefer `cargo add` / editing `Cargo.toml` for dependencies; run `cargo build` or `task build` after manifest changes
- `unsafe_code` is **forbidden** and `unwrap`/`expect`/`todo`/`dbg!` are **denied** via `Cargo.toml` `[lints]` — applies to all code including tests

## Git Conventions

- When asked to commit without a specific format, follow Conventional Commits: `<type>(<scope>): <imperative summary>`
- Never use `--no-verify` when committing or pushing; fix the underlying hook failure instead

## Documentation Scope

<!-- Keep this file limited to always-on repository rules. -->
- Read `docs/coding.md` before writing or modifying any Rust code.
- Read `docs/testing.md` before writing or modifying tests.
- Read `docs/tooling.md` when working with build, CI, hooks, or adding tools.
- Read `docs/review.md` when performing code review.
- Read `docs/adr/` only when historical rationale matters.
