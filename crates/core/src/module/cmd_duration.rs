//! Command duration module — displays how long the last command took.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

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
    let total_secs = ms / 1000;
    let minutes = total_secs / 60;
    let secs = total_secs % 60;
    if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
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
        assert_eq!(output.map(|o| o.content), Some("1m 5s".to_owned()));
    }

    #[test]
    fn test_module_exact_minute() {
        let ctx = make_ctx(Some(120_000));
        let output = CmdDurationModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("2m 0s".to_owned()));
    }
}
