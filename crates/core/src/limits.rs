//! Every resource limit in the runtime contract, defined once.
//!
//! The limits table in `docs/threat-model.md` §6 names each constant here;
//! `crates/cli/tests/docs.rs` fails when a row, a value, or a constant is
//! missing on either side. Limits apply per worker, not host-wide.

use std::time::Duration;

pub use capsule_protocol::session::{MAX_REQUEST, MAX_RESPONSE};

/// Maximum escaped snapshot (cwd and exported environment) the zsh integration
/// sends; a larger snapshot omits acquisition instead of being truncated.
///
/// `init.zsh` repeats this number and `crates/cli/tests/docs.rs` checks it.
pub const MAX_SNAPSHOT_BYTES: usize = 261_000;

/// Escaped request header before the snapshot: the tag, the four numbers at
/// their widest, separators, and up to 1 KiB of keymap name.
const MAX_REQUEST_HEADER_BYTES: usize = "Q\t".len()
    + "18446744073709551615\t".len()
    + "-2147483648\t".len()
    + "18446744073709551615\t".len()
    + "65535\t".len()
    + 1024
    + "\t".len();

const _: () = assert!(MAX_SNAPSHOT_BYTES + MAX_REQUEST_HEADER_BYTES <= MAX_REQUEST);

/// Maximum bytes retained for one command, file, or exported environment value.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Maximum TOML input size accepted before deserialization.
pub const MAX_CONFIG_BYTES: usize = MAX_OUTPUT_BYTES;

/// Maximum concurrent subprocesses per worker.
pub const MAX_PROCESSES: usize = 4;
/// Maximum concurrent blocking filesystem jobs per worker; also the size of
/// the worker runtime's blocking thread pool.
pub const MAX_FILE_JOBS: usize = 2;

/// Deadline for one acquisition I/O operation, including the wait for capacity.
pub const ACQUISITION_TIMEOUT: Duration = Duration::from_millis(500);
/// Deadline for one acquisition generation before it is cancelled and settled.
pub const GENERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Grace period for terminating and reaping a command's process group.
pub const CLEANUP_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum number of custom modules in a plan.
pub const MAX_MODULES: usize = 64;
/// Maximum named values across the entire plan.
pub const MAX_TOTAL_VALUES: usize = 256;
/// Maximum fallback candidates across the entire plan.
pub const MAX_TOTAL_CANDIDATES: usize = 1024;
/// Maximum named values in one module.
pub const MAX_VALUES_PER_MODULE: usize = 16;
/// Maximum fallback candidates for one named value.
pub const MAX_CANDIDATES_PER_VALUE: usize = 8;
/// Maximum entries in one `when.files` or `when.env` list.
pub const MAX_WHEN_ITEMS: usize = 64;
/// Maximum argv elements of one command candidate.
pub const MAX_COMMAND_ARGS: usize = 64;

/// Maximum format source length.
pub const MAX_FORMAT_BYTES: usize = 4096;
/// Maximum nested optional format sections.
pub const MAX_FORMAT_DEPTH: usize = 8;
/// Maximum compiled size of one candidate regex.
pub const MAX_REGEX_BYTES: usize = 64 * 1024;
/// Maximum syntactic nesting of one candidate regex.
pub const MAX_REGEX_NESTING: u32 = 32;

/// Maximum expanded text of one module format, so repeated references cannot
/// multiply bounded values into an unbounded allocation.
pub const MAX_MODULE_TEXT_BYTES: usize = 64 * 1024;
/// Maximum plain text per prompt line before styling, prompt quoting, and wire
/// escaping, which expand a backslash to 11 bytes; keeps both lines within
/// [`MAX_RESPONSE`].
pub const MAX_LINE_TEXT_BYTES: usize = 2 * 1024;

/// Maximum cwd ancestors searched for a Git repository.
pub const MAX_REPOSITORY_DEPTH: usize = 128;
