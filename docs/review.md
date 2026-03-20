# Review Guide

Read this file when performing code review.

Prioritize behavior, safety, regressions, and missing tests over style.

| Viewpoint | Check | Red Flags |
|---|---|---|
| API and spec alignment | Compare doc comments, examples, defaults, and error contracts against the implementation. | Comments promise behavior the code does not implement; hidden defaults; panics in library code. |
| Ownership and borrowing | Verify parameter types match usage — `&str` not `&String`, `&[T]` not `&Vec<T>`. Check that owned vs borrowed is intentional, not accidental. | Unnecessary cloning to satisfy the borrow checker; forcing callers into allocations; lifetime annotations that could be elided. |
| Type safety | Enums and newtypes used where raw primitives or strings encode domain semantics. Invalid states are unrepresentable. | Boolean flags for multi-state logic; stringly-typed variants; sentinel values instead of `Option`/`Result`. |
| Naming and API shape | Names follow `as_`/`to_`/`into_` conventions. No `get_` prefix on getters. Standard traits (`Debug`, `Clone`, `PartialEq`, etc.) derived where meaningful. | Inconsistent naming; public types that expose more than callers need; missing standard trait impls on public types. |
| Error handling | Errors carry context where useful; `?` used consistently; no silent drops. Library code returns `Result`, application code may use `anyhow`. Error messages lowercase, no trailing punctuation. | `unwrap`/`expect` without `#[expect(clippy::unwrap_used)]` and justification; errors that lose the cause without reason; panics in library code without `# Panics` docs. |
| Control flow | Early returns and `?` keep nesting shallow; iterators preferred over manual index loops where readable; resource cleanup is explicit (`Drop`, guards). | Easy-to-miss branches; off-by-one in manual loops; leaks on error paths. |
| Concurrency and async | Clear ownership of tasks and channels; shutdown or join paths documented when non-obvious. No blocking calls inside async tasks. | Fire-and-forget tasks without cancellation; blocking I/O in async context; shared mutable state without a documented protocol. |
| Resource lifecycle | Files, sockets, and handles have clear open/close or drop semantics. | Callers must know hidden cleanup rules not stated in the API. |
| Memory and allocations | Pre-allocation used for known sizes; unnecessary `format!`/`clone` avoided; zero-copy where straightforward. | Gratuitous allocations in hot paths; `clone()` used as a borrow-checker escape hatch. |
| Tests | Error paths and regressions covered; failures show expected vs actual. Doc tests validate public API examples. Tests use `Result`-returning functions with `?`. | Missing tests for risky branches; flaky timing-dependent tests; bare `unwrap`/`expect` without lint suppression. |
