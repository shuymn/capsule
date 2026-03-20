# Testing Conventions

Read this file before writing or modifying tests in this repository.

## Running Tests

- Use `task test` for the full suite (unit, integration, and doctests via `cargo test`).
- Use `task check` for CI-equivalent local verification (includes `cargo doc` without dependency docs).
- For focused runs: `cargo test -p rust-template <filter>` or `cargo test <name> -- --nocapture` as needed.

## Suite Expectations

- Tests should be deterministic and avoid reliance on execution order unless explicitly serialized.
- Prefer small, fast unit tests; use integration tests under `tests/` for boundary behavior.
- Doctests validate examples in `///` comments; keep them minimal and runnable.

## Test Organization

- Unit tests go in a `#[cfg(test)] mod tests` submodule at the bottom of the file.
- Use `use super::*` in test modules to access private items.
- Integration tests under `tests/` test public API only.
- Shared test helpers go in `tests/common/mod.rs` (not `tests/common.rs`, which Cargo treats as a test binary).

## Writing Tests

- Use `#[test]` functions that return `Result<(), E>` with `?` for cleaner error propagation instead of scattering `unwrap()`.
- Use `assert_eq!(actual, expected)` and `assert_ne!` — they show both values on failure. Include a message argument when the assertion is not self-explanatory.
- Test error paths and edge cases, not just happy paths.
- Name test functions descriptively: `test_parse_returns_error_on_empty_input`, not `test1`.
- For tests that need setup/teardown, use helper functions or RAII guards (Drop-based cleanup).
- Avoid `#[ignore]` without a comment explaining why and when the test should be un-ignored.

## Doc Tests

- Use `?` instead of `unwrap()` in doc examples.
- Use `# ` prefix to hide boilerplate (imports, main wrapper) while keeping examples compilable.
- Use `no_run` for examples that require external resources; `compile_fail` to demonstrate invalid usage.
