# Coding Conventions

Read this file before writing or modifying any Rust code in this repository.

## Ownership and Borrowing

- Accept `&str` instead of `&String`, `&[T]` instead of `&Vec<T>` — more general and avoids forcing callers into specific container types.
- Accept owned types (`String`, `Vec<T>`) only when the function needs to store or move the data.
- For flexible ownership transfer, accept `impl AsRef<str>` when borrowing suffices. Use `impl Into<String>` sparingly — only when the function genuinely needs ownership and callers hold diverse types. Prefer accepting `String` directly if most callers already have one; the added genericity of `Into<String>` rarely justifies the API complexity.
- Use `Cow<'a, str>` when a function sometimes borrows and sometimes needs to allocate.
- Prefer restructuring code over cloning to satisfy the borrow checker — pass callbacks, return values, or use indices into a single owning collection.
- Clone is acceptable for prototypes, non-hot-path code, and `Arc`/`Rc` (which are cheap reference-count increments).
- Watch for cloning in error construction — `.clone()` into error enum fields inside `.map_err()` is a common LLM pattern. Consider taking ownership where possible, using `Cow`, or accepting that the error path can allocate if it's truly exceptional.
- Use `std::mem::replace` and `Option::take` to swap values in place without moving.
- Avoid self-referential structs; store `Range<usize>` indices into owned data instead of internal references.
- Keep lifetime annotations minimal. If lifetime complexity is growing, restructure to use owned data or indices.

## Type System

- **Newtype pattern**: Wrap primitive types to encode domain semantics (`struct Meters(f64)` instead of `type Meters = f64`). Add `#[repr(transparent)]` for FFI. Implement `From`/`Into` for ergonomic conversions.
- **Typestate pattern**: Encode valid states and transitions as zero-sized marker types in generic parameters. Invalid transitions become compile errors at zero runtime cost.
- **Enums over boolean flags**: Use enums to make invalid states unrepresentable. Avoid stringly-typed variants.
- **Builder pattern**: Use for complex construction with many optional parameters. Validate at build time, not after construction.
- **Generics vs trait objects**: Prefer generics (`impl Trait` / `<T: Trait>`) by default for monomorphized, zero-cost dispatch. Use `dyn Trait` when heterogeneous collections or dynamic dispatch are genuinely needed. Do not default to `Box<dyn Trait>` for dependency injection when the concrete type is statically known — generics give the compiler more optimization opportunities and avoid heap allocation.
- **Derive macros over hand-written match**: Do not hand-write match arms to convert enum variants to strings. Use `strum` derive macros (`Display`, `EnumString`, etc.) to automate this. Add the dependency when needed.

## Error Handling

- Propagate errors with `Result` and the `?` operator. `unwrap`/`expect` are denied by Clippy in all code including tests. Use `Result`-returning functions with `?` instead. When `unwrap`/`expect` is genuinely unavoidable, add `#[expect(clippy::unwrap_used)]` with a justification comment.
- **Library code**: Define concrete error enums with `thiserror`. Each variant represents a distinct failure mode. Use `#[source]` or `#[from]` to preserve error chains. Prefer enums or newtypes over `String` for error variant context fields — keep error context structured.
- **Application code**: Use `anyhow::Result` for unified error handling with `.context()` for actionable messages.
- Use `thiserror` for library error types and `anyhow` for application error handling. Add these dependencies when needed — do not avoid them just because the project does not depend on them yet.
- Define a type alias when `Box<dyn std::error::Error + Send + Sync>` appears in multiple places.
- Use `Option<T>` when a value may be absent with no error information needed; `Result<T, E>` when the caller needs to know why.
- Prefer `?` and combinators (`map`, `map_err`, `and_then`, `unwrap_or_else`) over explicit `match` on `Result`/`Option`.
- Collect `Result`s with `collect::<Result<Vec<_>, _>>()?` to short-circuit on first error.
- **When to panic**: Only for programmer errors (violated invariants, unreachable states). Libraries should almost never panic. When a panic is intentional, use `#[expect(clippy::unwrap_used)]` with `expect("reason")` to document the invariant.
- **Error messages**: Lowercase, no trailing punctuation, concise. Example: `"unexpected end of file"`.
- Do not swallow errors silently; log or return at the boundary.

## API Design

- Keep public API surface small and documented with `///` where behavior is not obvious.
- **Naming conventions**:
  - `as_` — free conversion, borrows receiver (`as_bytes`)
  - `to_` — expensive conversion, may allocate (`to_lowercase`)
  - `into_` — consumes receiver, returns owned value (`into_bytes`)
  - Omit `get_` prefix for getters: `fn field(&self) -> &T` and `fn field_mut(&mut self) -> &mut T`
  - Iterators: `iter()`, `iter_mut()`, `into_iter()` with type names `Iter`, `IterMut`, `IntoIter`
  - Constructors: `new()` or `with_details()`. Use `from_other_type()` for conversion constructors.
- **Standard traits**: Implement `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Default`, and `Display` for public types where meaningful. Derive where possible.
- **Conversions**: Implement `From<T>` (not `Into` directly). Use `TryFrom` for fallible conversions. Implement `AsRef<T>` and `AsMut<T>` for cheap reference conversions.
- Return values instead of mutating out-parameters.
- Sealed traits for traits not intended for downstream implementation.

## Memory and Performance

- Pre-allocate collections when size is known: `Vec::with_capacity(n)`, `String::with_capacity(n)`.
- Reuse allocations: `vec.clear()` instead of creating a new `Vec` in loops. Use `clone_from()` to reuse existing buffers.
- Avoid unnecessary `format!()` — it always allocates. Prefer `write!` to an existing buffer or string literals.
- Remove `.clone()` calls that became unnecessary after refactoring.
- Avoid repeated `.to_string()` / `.to_owned()` inside loops; consider processing with `Cow<str>` or references instead.
- Use zero-copy parsing: `&str` slices into the original input rather than allocating new `String`s for each token.
- Use `smallvec::SmallVec<[T; N]>` for collections that are usually small, when profiling shows benefit. Add the dependency when needed.
- Profile before optimizing; measure before and after.

## Async and Concurrency

- **Prefer synchronous code** unless the task genuinely benefits from async I/O.
- Never call blocking operations (file I/O, CPU-heavy computation) inside async tasks — use `spawn_blocking` or a dedicated thread pool.
- Prefer async-native libraries (`tokio::fs` over `std::fs`) inside async contexts.
- When using `async`, document cancellation and runtime expectations if non-obvious.
- Prefer structured concurrency (scoped tasks, `JoinSet`) over detached tasks without shutdown.
- **Shared state**: Use `Arc<Mutex<T>>` for low-contention shared mutable state. Use `Arc<RwLock<T>>` when reads vastly outnumber writes. Prefer channels for message-passing architectures.
- Use `Weak<T>` for back-pointers to avoid reference cycles with `Rc`/`Arc`.

## Module Organization

- Default to private. Use `pub(crate)` for crate-internal helpers. Expose `pub` only for intentional API surface.
- Use `pub use` re-exports to flatten internal module structure into a clean public API.
- Avoid glob imports except `use super::*` in test modules.
- One concept per module — usually one primary type with its impls. Avoid catch-all `util` modules.
- Place public items before private items in files.
- Enum variants and trait methods are always public if the enum/trait is public.

## Documentation

- Add `//!` crate-level docs in `lib.rs`: brief description, usage example.
- Document every public item with `///`. Structure: one-line summary, details, `# Examples`, `# Errors`, `# Panics`, `# Safety` (for `unsafe`).
- Doc tests are compiled and run by `cargo test`. Use `?` not `unwrap()` in examples. Use `# ` prefix to hide boilerplate.
- Use intra-doc links (`[TypeName]`) to cross-reference types and functions.

## Anti-patterns to Avoid

- **Cloning to satisfy the borrow checker** — restructure ownership instead.
- **Stringly-typed code** — use enums and newtypes for categories, states, and identifiers. This includes error context fields — prefer structured types over `String`.
- **Overusing `Rc<RefCell<T>>`** — usually signals a design problem. Restructure with callbacks, returned values, or indices.
- **Sentinel values** — return `Option<T>` or `Result<T, E>` instead of magic values like `-1` or empty strings.
- **Initialize-then-populate** — construct objects fully initialized with constructors or builders.
- **Manual index loops** — prefer iterators. They eliminate off-by-one errors and often generate better machine code.
- **Using `unsafe` to fight the borrow checker** — refactor the code instead.
- **Defaulting to `Box<dyn Trait>`** — use generics when the concrete type is statically known. Do not reach for trait objects solely for dependency injection.
- **Excessive `impl Into<T>` parameters** — if callers use only one or two types, accept `String` or `&str` directly. The added genericity rarely justifies the API complexity.
- **Hand-written enum-to-string match** — use derive macros like `strum` to automate variant name conversions instead of maintaining match arms manually.
- **Repeated allocations in loops** — avoid calling `.to_string()` per iteration; process with `Cow` or references instead.
- **Repeating verbose type patterns** — define a type alias when `Box<dyn Error + Send + Sync>` or similar appears in multiple places.

## Recommended Libraries

Add these dependencies when the use case arises. Do not reimplement what they provide, and do not avoid them just because the project does not depend on them yet.

| Use case | Crate | When to add |
|---|---|---|
| Library error types | `thiserror` | Any crate that defines its own error enums |
| Application error handling | `anyhow` | Binary crates or top-level error propagation |
| Enum derive utilities | `strum` | Enum-to-string, string-to-enum, iteration over variants |
| Serialization | `serde` + format crate (`serde_json`, `toml`, etc.) | Any struct that crosses a serialization boundary |
| CLI argument parsing | `clap` (derive) | Binary crates with CLI arguments |
| Async runtime | `tokio` | Async I/O (prefer single-threaded `current_thread` unless concurrency is needed) |
| HTTP client | `reqwest` | Outbound HTTP requests |
| Logging | `tracing` | Structured logging and diagnostics |
| Small inline collections | `smallvec` | When profiling shows many short-lived small `Vec`s |

## Style

- Run `task fmt` before committing. Formatting uses **nightly** `rustfmt` with unstable options enabled in [rustfmt.toml](../rustfmt.toml): import grouping (`StdExternalCrate`), crate-level import granularity, impl item reordering, wildcard suffix condensing, and formatting inside doc comments / macro matchers. Do not strip `unstable_features` without replacing the dependent options.
- Clippy: `task lint` runs `cargo clippy ... -- -D warnings`. See [docs/tooling.md](../docs/tooling.md) for the full Clippy policy (lint sources, thresholds, crate-level attributes). When you add a library crate root, copy the `#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]` attributes from `src/main.rs` to the new `lib.rs`.
- Prefer iterators over explicit loops when the transform chain is readable.
- When a loop body is large or multi-step logic doesn't map naturally to closures, a plain `for` loop is more readable.
- Use `usize` for indices, not `i32` or `u32`.
