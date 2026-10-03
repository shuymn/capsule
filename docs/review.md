# Review guide

Read when reviewing code. Prioritize behavior, lifecycle safety, regressions, and missing verification. Apply [coding.md](coding.md) without duplicating formatter or Clippy diagnostics.

- Check public behavior, defaults, examples, and errors against [architecture.md](architecture.md) and the implementation.
- Trace generation changes, environment replacement, partial I/O, cancellation, resource bounds, and child cleanup through success and failure paths.
- Verify configuration validation, ordered fallback, arbitration, grapheme layout, and prompt-data escaping at their owning boundaries.
- Check that APIs expose only necessary state, ownership is explicit, and errors preserve useful context.
- Require tests for changed contracts and concrete regressions; use [testing.md](testing.md) to distinguish unit checks from real shell/platform evidence.

Report actionable findings with the triggering case, user-visible consequence, and source location. Identify unverified behavior separately from confirmed failures.
