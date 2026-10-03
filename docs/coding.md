# Coding conventions

Read before modifying Rust. Use [architecture.md](architecture.md) for runtime contracts, [testing.md](testing.md) for tests, and [tooling.md](tooling.md) for formatting and lint policy.

## Ownership and APIs

- Accept borrowed slices/references unless the callee stores or moves the value. Add `AsRef` or `Into` only for actual caller needs.
- Keep ownership and lifetimes simple; use returned values, indices, or ranges instead of self-references and borrow-checker-driven cloning. Use `Cow` when ownership varies; cheap `Arc` clones are appropriate for shared ownership.
- Default to private; use `pub(crate)` for internal helpers and expose only intentional public APIs. Avoid catch-all utility modules and glob imports outside tests.
- Prefer concrete types and generics when types are known. Use trait objects for heterogeneous runtime values, not solely for dependency injection.
- Represent domain states with enums/newtypes and absence/failure with `Option`/`Result`. Use builders or typestate only where construction/state constraints require them.
- Follow Rust naming and conversion traits (`as_`, `to_`, `into_`, `From`, `TryFrom`); omit `get_` on getters. Derive standard traits where meaningful and use `strum` for enum conversions.

## Errors and documentation

- Use structured `thiserror` enums in libraries and `anyhow` with actionable context at application boundaries. Preserve causes with `source`/`from`; propagate with `?` and report errors at their owner boundary.
- Write concise lowercase error messages without trailing punctuation. Add an alias for repeated verbose error types.
- Follow the workspace's denied panic/unwrap/expect/unsafe lints. Document any justified lint expectation with the matching lint name; panics must represent programmer invariants.
- Document public items with `///` and crates with `//!`. Include relevant error, panic, cancellation, and runtime contracts; use intra-doc links and executable examples where useful.

## Resources and concurrency

- Keep filesystem/blocking work off the async runtime and retain bounded capacity until that work actually finishes. Use the shared Runner for acquisition.
- Own spawned tasks and their shutdown through structured concurrency. Document non-obvious cancellation and channel protocols.
- Use channels for message passing and explicit ownership for shared state. Keep cleanup effective on errors and dropped futures.
- Reuse allocations and borrow parsed data where straightforward. Choose readable loops or iterators; use `usize` for indices. Make performance changes from measurements under [benchmarking.md](benchmarking.md).

## Libraries

Use established crates for their existing roles: `serde` and format crates for serialization, `clap` for CLI parsing, `tracing` for diagnostics, `tokio`/`tokio-util` for runtime/framing/cancellation, rustix for OS operations, and Unicode crates for segmentation/width. Add dependencies for a concrete need; keep interfaces and maintained code small.
