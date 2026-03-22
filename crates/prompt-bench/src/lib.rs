//! Shared helpers for the prompt benchmark binary (stats, PATH construction, report types).

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]
// `capsule-protocol` pulls `tokio`, which pins a different `hashbrown` than other workspace edges.
#![allow(clippy::multiple_crate_versions)]

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Default samples per workload (excluding warm-up).
pub const DEFAULT_ITERATIONS: usize = 30;

/// Max wait for the first response line (`RenderResult`) per request.
pub const RENDER_RESULT_WAIT_SECS: u64 = 10;

/// Max wait for an optional second line (`Update`) after `RenderResult`.
///
/// The daemon omits `Update` when slow modules do not change the composed prompt (typical for
/// non-repository paths). A multi-second wait here would stall the whole workload on every
/// iteration; this cap must stay above real slow-path latency for the benchmark repos (see
/// `docs/benchmarking.md`).
pub const UPDATE_WAIT_MS: u64 = 250;

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
    let stddev_ms = sample_stddev(values);
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
fn sample_stddev(values: &[f64]) -> f64 {
    let n = values.len();
    if n <= 1 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    let sum_sq: f64 = values.iter().map(|x| (x - mean).powi(2)).sum();
    (sum_sq / (n as f64 - 1.0)).sqrt()
}

/// One tool × workload row in the report.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub tool: String,
    pub workload: String,
    pub description: String,
    pub fast: SummaryStats,
    pub slow: Option<SummaryStats>,
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

/// Resolve `name` on `PATH` or return `path` if it exists as a file.
///
/// # Errors
///
/// Returns an error if the binary cannot be resolved.
pub fn resolve_binary(path_or_name: &Path, label: &str) -> anyhow::Result<PathBuf> {
    if let Some(p) = which(path_or_name) {
        return Ok(p);
    }
    if path_or_name.is_file() {
        return path_or_name
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("{label} not found: {}: {e}", path_or_name.display()));
    }
    anyhow::bail!("{label} not found: {}", path_or_name.display())
}

fn which(name: &Path) -> Option<PathBuf> {
    let file_name = name.file_name()?.to_str()?;
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(file_name);
        if candidate.is_file() && is_executable(&candidate) {
            return candidate.canonicalize().ok();
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

/// Build `PATH` for the benchmark daemon: capsule, starship, git, optional rustc, plus `/usr/bin` and `/bin`.
#[must_use]
pub fn build_path_env(
    capsule_bin: &Path,
    starship_bin: &Path,
    git_bin: &Path,
    rustc_bin: Option<&Path>,
) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut bin_dirs: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for b in [capsule_bin, starship_bin, git_bin] {
        if let Some(parent) = b.parent()
            && seen.insert(parent.to_path_buf())
        {
            bin_dirs.push(parent.to_path_buf());
        }
    }
    if let Some(r) = rustc_bin
        && let Some(parent) = r.parent()
        && seen.insert(parent.to_path_buf())
    {
        bin_dirs.push(parent.to_path_buf());
    }
    for d in [Path::new("/usr/bin"), Path::new("/bin")] {
        if seen.insert(d.to_path_buf()) {
            bin_dirs.push(d.to_path_buf());
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
    }

    #[test]
    #[cfg(unix)]
    fn path_env_deduplicates_directories() {
        let path = build_path_env(
            Path::new("/opt/homebrew/bin/capsule"),
            Path::new("/opt/homebrew/bin/starship"),
            Path::new("/opt/homebrew/bin/git"),
            None,
        );
        let parts: Vec<&str> = path.split(':').collect();
        assert!(parts.contains(&"/bin"));
        assert!(parts.contains(&"/usr/bin"));
        let unique: std::collections::HashSet<_> = parts.iter().copied().collect();
        assert_eq!(unique.len(), parts.len());
    }
}
