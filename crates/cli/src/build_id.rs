//! Binary fingerprint computation.

use std::{fmt::Write as _, os::unix::fs::MetadataExt as _};

/// Compute a fingerprint for the current binary.
///
/// Returns `file_size:mtime_nanos` derived from `current_exe()` metadata.
/// Returns `None` if the executable path or metadata cannot be read.
pub fn compute() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    let size = meta.len();
    let secs: u64 = meta.mtime().try_into().ok()?;
    let nsec: u64 = meta.mtime_nsec().try_into().ok()?;
    let mtime_nanos = secs * 1_000_000_000 + nsec;
    let mut id = String::with_capacity(32);
    let _ = write!(id, "{size}:{mtime_nanos}");
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_returns_some() {
        assert!(compute().is_some(), "should succeed for the test binary");
    }

    #[test]
    fn test_compute_format() {
        let id = compute();
        assert!(id.is_some(), "should succeed for the test binary");
        let id = id.unwrap_or_default();
        let parts: Vec<&str> = id.splitn(2, ':').collect();
        assert_eq!(parts.len(), 2, "should have format 'size:mtime_nanos'");
        assert!(
            parts[0].parse::<u64>().is_ok(),
            "size should be a valid u64"
        );
        assert!(
            parts[1].parse::<u64>().is_ok(),
            "mtime_nanos should be a valid u64"
        );
    }

    #[test]
    fn test_compute_is_deterministic() {
        let id1 = compute();
        let id2 = compute();
        assert_eq!(id1, id2, "consecutive calls should return the same value");
    }
}
