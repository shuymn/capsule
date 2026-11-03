<!-- Maintenance: Update when tasks, hooks, or project scope changes. -->
<!-- Audience: All docs under docs/ and this file are written for coding agents (LLMs), not humans. Use direct instructions, not tutorials or explanations of concepts the agent already knows. Apply this rule when creating or updating any documentation. -->

## Build, Test, and Development Commands

- Use mise tasks ([mise.toml](mise.toml)) as the default interface; mise also pins lefthook and uv
- `mise run build` / `mise run test` / `mise run lint` / `mise run fmt` / `mise run check` — primary workflow; `mise run check` runs formatting check, Clippy, tests, `cargo doc`, and build; `mise run check:fast` skips tests and docs (see [docs/tooling.md](docs/tooling.md))
- `mise run release:check` validates a checked-out release candidate without side effects; `mise run release:test` exercises the release contract (see [docs/releasing.md](docs/releasing.md))
- Rust-native equivalents work without mise: `cargo build`, `cargo test`, `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` (same as `mise run lint`; see [docs/tooling.md](docs/tooling.md) for Clippy policy details)
- Prefer `cargo add` / editing `Cargo.toml` for dependencies; run `cargo build` or `mise run build` after manifest changes
- `unsafe_code` is **forbidden** and `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!`/`dbg!` are **denied** via `Cargo.toml` `[lints]` — applies to all code including tests

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
- Add or change a Behavioral contract clause in `docs/architecture.md` property first: write the named property (or, for process/shell behavior, the contract or PTY test), then cite it in the clause's `Spec:`.
