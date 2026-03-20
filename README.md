# rust-template

<!-- template:start -->
Minimal Rust template with:

- [Taskfile.yml](Taskfile.yml) as the primary entrypoint for local tasks, CI, and hooks
- **Nightly** toolchain with opinionated [rustfmt.toml](rustfmt.toml), [clippy.toml](clippy.toml), and `Cargo.toml` `[lints]`; `clippy::pedantic` / `nursery` / `cargo` enabled at the crate root
- [lefthook.yml](lefthook.yml) for `pre-commit` and `pre-push` automation
- GitHub Actions running the same `task` commands as local development
- Starter docs for repository rules and review conventions

Template source: `https://github.com/shuymn/rust-template` (`shuymn/rust-template`).
<!-- template:end -->

This repository was initialized from a Rust project template.

Replace this README with project-specific documentation once the repository has a clear purpose, setup flow, and release process.

## Local Setup

Install [Rust](https://www.rust-lang.org/tools/install) (rustup). This repo pins **nightly** in `rust-toolchain.toml` (run `rustup show` in the repo root after clone — rustup should install it automatically). Optional: [Task](https://taskfile.dev/installation/) and [Lefthook](https://github.com/evilmartians/lefthook).

Primary entrypoint:

```bash
task
task build
task test
task lint
task fmt
task check
```

After installing Lefthook, enable hooks:

```bash
lefthook install
```

Rust-native equivalents (same nightly toolchain as `rust-toolchain.toml`):

```bash
cargo build
cargo test
cargo fmt --all -- --check   # uses nightly rustfmt + rustfmt.toml
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

## Initial Customization

Before treating this as a real project, update the repository-specific parts:

1. Run template initialization from the repository root. This rewrites template placeholders, removes the template-only README section, deletes `.taskfiles/`, and creates a local commit.

```bash
task -t .taskfiles/template.yml init
```

2. Replace [`src/main.rs`](src/main.rs) and any starter code with your actual application.
3. Rewrite this README with your project's purpose, setup, development workflow, and release information.
4. Review [`AGENTS.md`](AGENTS.md) and [`docs/`](docs/) and keep only the rules and guidance you want in this repository.
5. Run `task check` before your first project-specific commit (requires Lefthook on PATH for the `install:lefthook` dependency, or run `task fmt:check`, `task lint`, `task test`, `task doc`, and `task build` individually).

## Suggested README Sections

When you rewrite this file, include only the sections your project actually needs, for example:

- Project overview
- Requirements
- Setup
- Local development commands
- Testing
- Deployment or release process
- Repository layout
- Links to deeper docs if needed
