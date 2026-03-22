//! Command duration module — displays how long the last command took.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::sealed;

/// Default threshold in milliseconds below which duration is not shown.
const DEFAULT_THRESHOLD_MS: u64 = 2000;

/// Displays the duration of the last command when it exceeds the threshold.
///
/// Returns `None` when duration is absent or below the threshold.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct CmdDurationModule {
    threshold_ms: u64,
}

impl Default for CmdDurationModule {
    fn default() -> Self {
        Self::new()
    }
}

impl CmdDurationModule {
    /// Creates a new `CmdDurationModule` with the default threshold.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            threshold_ms: DEFAULT_THRESHOLD_MS,
        }
    }

    /// Creates a new `CmdDurationModule` with a custom threshold.
    #[must_use]
    pub const fn with_threshold(threshold_ms: u64) -> Self {
        Self { threshold_ms }
    }
}

impl sealed::Sealed for CmdDurationModule {}

impl Module for CmdDurationModule {
    fn name(&self) -> &'static str {
        "cmd_duration"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let ms = ctx.duration_ms?;
        if ms < self.threshold_ms {
            return None;
        }
        Some(ModuleOutput {
            content: format_duration(ms),
        })
    }
}

fn format_duration(ms: u64) -> String {
    use std::fmt::Write;

    let total_secs = ms / 1000;
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    let mut buf = String::new();
    if days > 0 {
        let _ = write!(buf, "{days}d");
    }
    if hours > 0 {
        let _ = write!(buf, "{hours}h");
    }
    if minutes > 0 {
        let _ = write!(buf, "{minutes}m");
    }
    if buf.is_empty() || secs > 0 {
        let _ = write!(buf, "{secs}s");
    }
    buf
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn make_ctx(duration_ms: Option<u64>) -> RenderContext<'static> {
        RenderContext {
            cwd: Path::new("/tmp"),
            home_dir: Path::new("/Users/testuser"),
            last_exit_code: 0,
            duration_ms,
            keymap: "main",
            cols: 80,
        }
    }

    #[test]
    fn test_module_none_duration_returns_none() {
        let ctx = make_ctx(None);
        assert!(CmdDurationModule::new().render(&ctx).is_none());
    }

    #[test]
    fn test_module_below_threshold_returns_none() {
        let ctx = make_ctx(Some(1999));
        assert!(CmdDurationModule::new().render(&ctx).is_none());
    }

    #[test]
    fn test_module_at_threshold() {
        let ctx = make_ctx(Some(2000));
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("2s".to_owned()));
    }

    #[test]
    fn test_module_above_threshold() {
        let ctx = make_ctx(Some(3500));
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("3s".to_owned()));
    }

    #[test]
    fn test_module_minutes_and_seconds() {
        let ctx = make_ctx(Some(65_000));
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("1m5s".to_owned()));
    }

    #[test]
    fn test_module_exact_minute() {
        let ctx = make_ctx(Some(120_000));
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("2m".to_owned()));
    }

    #[test]
    fn test_module_hours_and_minutes() {
        let ctx = make_ctx(Some(3_661_000)); // 1h 1m 1s
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("1h1m1s".to_owned()));
    }

    #[test]
    fn test_module_exact_hour() {
        let ctx = make_ctx(Some(3_600_000)); // 1h
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("1h".to_owned()));
    }

    #[test]
    fn test_module_days() {
        let ctx = make_ctx(Some(90_061_000)); // 1d 1h 1m 1s
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("1d1h1m1s".to_owned()));
    }

    #[test]
    fn test_module_exact_day() {
        let ctx = make_ctx(Some(86_400_000)); // 1d
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("1d".to_owned()));
    }
}
