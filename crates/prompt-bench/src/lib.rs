//! Shared helpers for the prompt benchmark binary (stats, PATH construction, report types).

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Default samples per workload (excluding warm-up).
pub const DEFAULT_ITERATIONS: usize = 30;

/// Max wait for the worker's explicit completion response or a subprocess.
///
/// Missing completion never counts as a successful sample, even when an initial
/// response has already arrived.
pub const ACQUISITION_WAIT_SECS: u64 = 10;

/// Linear interpolation percentile on a **pre-sorted** slice, matching the `CPython`
/// `statistics` linear method.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]
fn percentile_sorted(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = (sorted.len() - 1) as f64 * fraction;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let weight = position - lower as f64;
    (sorted[upper] - sorted[lower]).mul_add(weight, sorted[lower])
}

/// Summary statistics in milliseconds.
#[derive(Debug, Clone, Serialize)]
pub struct SummaryStats {
    pub count: usize,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub stddev_ms: f64,
}

/// Build summary stats; `stddev_ms` uses sample standard deviation (Bessel), like `statistics.stdev`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn summarize(values: &[f64]) -> SummaryStats {
    let count = values.len();
    let min_ms = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max_ms = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean_ms = if count == 0 {
        0.0
    } else {
        values.iter().sum::<f64>() / count as f64
    };
    let stddev_ms = sample_stddev(values, mean_ms);
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    SummaryStats {
        count,
        min_ms,
        p50_ms: percentile_sorted(&sorted, 0.50),
        p95_ms: percentile_sorted(&sorted, 0.95),
        max_ms,
        mean_ms,
        stddev_ms,
    }
}

#[allow(clippy::cast_precision_loss)]
fn sample_stddev(values: &[f64], mean: f64) -> f64 {
    let n = values.len();
    if n <= 1 {
        return 0.0;
    }
    let sum_sq: f64 = values.iter().map(|x| (x - mean).powi(2)).sum();
    (sum_sq / (n as f64 - 1.0)).sqrt()
}

/// One tool × workload row in the report.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub tool: String,
    pub workload: String,
    pub fast: SummaryStats,
    pub slow: Option<SummaryStats>,
    /// Verified completed rustc invocations in the measured samples (excludes warm-up).
    pub toolchain_acquisitions: usize,
}

/// Environment metadata recorded alongside results.
#[derive(Debug, Clone, Serialize)]
pub struct RunMetadata {
    pub iterations: usize,
    pub capsule_bin: String,
    pub starship_bin: String,
    pub git_bin: String,
    pub rustc: String,
    pub macos: String,
    pub kernel: String,
    pub cpu: String,
}

/// Resolve a bare name on `PATH`, or use an explicit executable path.
///
/// Preserve symlinks: tools such as rustup select behavior from the invoked filename.
///
/// # Errors
///
/// Returns an error if the binary cannot be resolved.
pub fn resolve_binary(path_or_name: &Path, label: &str) -> anyhow::Result<PathBuf> {
    if path_or_name.components().count() > 1 || path_or_name.is_absolute() {
        if path_or_name.is_file() && is_executable(path_or_name) {
            return std::path::absolute(path_or_name).map_err(|e| {
                anyhow::anyhow!("{label} not found: {}: {e}", path_or_name.display())
            });
        }
    } else if let Some(path) = which(path_or_name) {
        return Ok(path);
    }
    anyhow::bail!("{label} not found: {}", path_or_name.display())
}

fn which(name: &Path) -> Option<PathBuf> {
    let file_name = name.file_name()?.to_str()?;
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(file_name);
        if candidate.is_file() && is_executable(&candidate) {
            return std::path::absolute(candidate).ok();
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// Build the fixture `PATH`: capsule, starship, git, optional rustc, plus `/usr/bin` and `/bin`.
#[must_use]
pub fn build_path_env(
    capsule_bin: &Path,
    starship_bin: &Path,
    git_bin: &Path,
    rustc_bin: Option<&Path>,
) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut bin_dirs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let directories = [capsule_bin, starship_bin, git_bin]
        .into_iter()
        .chain(rustc_bin)
        .filter_map(Path::parent)
        .chain([Path::new("/usr/bin"), Path::new("/bin")]);
    for directory in directories {
        if seen.insert(directory) {
            bin_dirs.push(directory);
        }
    }
    bin_dirs
        .iter()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(&sep.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_sorted_interpolates() {
        let v = [10.0_f64, 20.0, 30.0, 40.0];
        assert!((percentile_sorted(&v, 0.95) - 38.5).abs() < 1e-9);
    }

    #[test]
    fn summarize_reports_expected_fields() {
        let v = [10.0_f64, 20.0, 30.0];
        let s = summarize(&v);
        assert_eq!(s.count, 3);
        assert!((s.min_ms - 10.0).abs() < f64::EPSILON);
        assert!((s.p50_ms - 20.0).abs() < f64::EPSILON);
        assert!((s.max_ms - 30.0).abs() < f64::EPSILON);
        assert!((s.mean_ms - 20.0).abs() < f64::EPSILON);
        assert!((s.stddev_ms - 10.0).abs() < f64::EPSILON);
        for values in [&[][..], &[20.0][..]] {
            assert!(summarize(values).stddev_ms.abs() < f64::EPSILON);
        }
    }

    #[test]
    #[cfg(unix)]
    fn path_env_preserves_priority_and_deduplicates_directories() {
        for (binaries, rustc, expected) in [
            (
                [
                    "/opt/homebrew/bin/capsule",
                    "/opt/homebrew/bin/starship",
                    "/opt/homebrew/bin/git",
                ],
                None,
                "/opt/homebrew/bin:/usr/bin:/bin",
            ),
            (
                [
                    "/tools/capsule/capsule",
                    "/usr/bin/starship",
                    "/tools/git/git",
                ],
                Some("/tools/rust/rustc"),
                "/tools/capsule:/usr/bin:/tools/git:/tools/rust:/bin",
            ),
            (
                ["capsule", "starship", "git"],
                Some("/rustc"),
                ":/:/usr/bin:/bin",
            ),
        ] {
            let [capsule, starship, git] = binaries.map(Path::new);
            assert_eq!(
                build_path_env(capsule, starship, git, rustc.map(Path::new)),
                expected
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn explicit_binary_path_preserves_multicall_symlink() -> anyhow::Result<()> {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let dir = tempfile::tempdir()?;
        let target = dir.path().join("proxy");
        std::fs::write(&target, "#!/bin/sh\nexit 0\n")?;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        let executable = dir.path().join("sh");
        symlink(&target, &executable)?;

        assert_eq!(resolve_binary(&executable, "test")?, executable);
        assert!(resolve_binary(&dir.path().join("missing/sh"), "test").is_err());
        Ok(())
    }
}
