//! Binary fingerprint computation.

use std::{fmt::Write as _, os::unix::fs::MetadataExt as _};

use capsule_protocol::BuildId;

/// Compute a fingerprint for the current binary.
///
/// Returns `file_size:mtime_nanos` derived from `current_exe()` metadata.
/// Returns `None` if the executable path or metadata cannot be read.
pub fn compute() -> Option<BuildId> {
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    let size = meta.len();
    let secs: u64 = meta.mtime().try_into().ok()?;
    let nsec: u64 = meta.mtime_nsec().try_into().ok()?;
    let mtime_nanos = secs * 1_000_000_000 + nsec;
    let mut id = String::with_capacity(32);
    let _ = write!(id, "{size}:{mtime_nanos}");
    Some(BuildId::new(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_format() {
        let id = compute();
        assert!(id.is_some(), "should succeed for the test binary");
        let id_str = id.map_or_else(String::new, |id| id.as_str().to_owned());
        let parts: Vec<&str> = id_str.splitn(2, ':').collect();
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
}
