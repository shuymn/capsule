//! Every resource limit in the runtime contract, defined once.
//!
//! The limits table in `docs/threat-model.md` §6 names each constant in
//! [`ALL`]; `crates/cli/tests/docs.rs` fails when a row, a value, or a constant
//! is missing on either side. Limits apply per worker, not host-wide.

use std::time::Duration;

pub use capsule_protocol::session::{MAX_REQUEST, MAX_RESPONSE};

/// A limit's value in the unit its documentation row states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// A size in bytes.
    Bytes(usize),
    /// A number of items, levels, or concurrent operations.
    Count(usize),
    /// A deadline or grace period.
    Time(Duration),
}

/// Define each limit as a constant and list it, with its unit, in [`ALL`].
macro_rules! limits {
    ($($(#[$doc:meta])* $kind:ident $name:ident: $type:ty = $value:expr;)*) => {
        $($(#[$doc])* pub const $name: $type = $value;)*

        /// Every limit by name, including the re-exported frame bounds.
        pub const ALL: &[(&str, Limit)] = &[
            ("MAX_REQUEST", Limit::Bytes(MAX_REQUEST)),
            ("MAX_RESPONSE", Limit::Bytes(MAX_RESPONSE)),
            $((stringify!($name), limits!(@value $kind $name)),)*
        ];
    };
    (@value Time $name:ident) => { Limit::Time($name) };
    (@value $kind:ident $name:ident) => { Limit::$kind($name as usize) };
}

/// Escaped request header before the snapshot: the tag, the four numbers at
/// their widest, separators, and up to 1 KiB of keymap name.
const MAX_REQUEST_HEADER_BYTES: usize = "Q\t".len()
    + "18446744073709551615\t".len()
    + "-2147483648\t".len()
    + "18446744073709551615\t".len()
    + "65535\t".len()
    + 1024
    + "\t".len();

limits! {
    /// Maximum escaped snapshot (cwd and exported environment) the zsh
    /// integration sends; a larger snapshot omits acquisition instead of being
    /// truncated. `init.zsh` repeats this number and a test checks it.
    Bytes MAX_SNAPSHOT_BYTES: usize = 261_000;
    /// Maximum bytes retained for one command, file, or exported environment value.
    Bytes MAX_OUTPUT_BYTES: usize = 64 * 1024;
    /// Maximum TOML input size accepted before deserialization.
    Bytes MAX_CONFIG_BYTES: usize = MAX_OUTPUT_BYTES;

    /// Maximum concurrent subprocesses per worker.
    Count MAX_PROCESSES: usize = 4;
    /// Maximum concurrent blocking filesystem jobs per worker; also the size of
    /// the worker runtime's blocking thread pool.
    Count MAX_FILE_JOBS: usize = 2;

    /// Deadline for one acquisition I/O operation, including the wait for capacity.
    Time ACQUISITION_TIMEOUT: Duration = Duration::from_millis(500);
    /// Deadline for one acquisition generation before it is cancelled and settled.
    Time GENERATION_TIMEOUT: Duration = Duration::from_secs(2);
    /// Grace period for terminating and reaping a command's process group.
    Time CLEANUP_TIMEOUT: Duration = Duration::from_millis(100);

    /// Maximum number of custom modules in a plan.
    Count MAX_MODULES: usize = 64;
    /// Maximum named values across the entire plan.
    Count MAX_TOTAL_VALUES: usize = 256;
    /// Maximum fallback candidates across the entire plan.
    Count MAX_TOTAL_CANDIDATES: usize = 1024;
    /// Maximum named values in one module.
    Count MAX_VALUES_PER_MODULE: usize = 16;
    /// Maximum fallback candidates for one named value.
    Count MAX_CANDIDATES_PER_VALUE: usize = 8;
    /// Maximum entries in one `when.files` or `when.env` list.
    Count MAX_WHEN_ITEMS: usize = 64;
    /// Maximum argv elements of one command candidate.
    Count MAX_COMMAND_ARGS: usize = 64;

    /// Maximum format source length.
    Bytes MAX_FORMAT_BYTES: usize = 4096;
    /// Maximum nested optional format sections.
    Count MAX_FORMAT_DEPTH: usize = 8;
    /// Maximum compiled size of one candidate regex.
    Bytes MAX_REGEX_BYTES: usize = 64 * 1024;
    /// Maximum syntactic nesting of one candidate regex.
    Count MAX_REGEX_NESTING: u32 = 32;

    /// Maximum expanded text of one module format, so repeated references
    /// cannot multiply bounded values into an unbounded allocation.
    Bytes MAX_MODULE_TEXT_BYTES: usize = 64 * 1024;
    /// Maximum plain text per prompt line before styling, prompt quoting, and
    /// wire escaping, which expand a backslash to 11 bytes; keeps both lines
    /// within [`MAX_RESPONSE`].
    Bytes MAX_LINE_TEXT_BYTES: usize = 2 * 1024;

    /// Maximum cwd ancestors searched for a Git repository.
    Count MAX_REPOSITORY_DEPTH: usize = 128;
}

const _: () = assert!(MAX_SNAPSHOT_BYTES + MAX_REQUEST_HEADER_BYTES <= MAX_REQUEST);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_snapshot_bound_matches_max_snapshot_bytes() {
        let bound = format!("(( ${{#_CAPSULE_SNAPSHOT}} > {MAX_SNAPSHOT_BYTES} ))");
        assert_eq!(
            include_str!("init/init.zsh").matches(&bound).count(),
            1,
            "{bound}"
        );
    }
}
