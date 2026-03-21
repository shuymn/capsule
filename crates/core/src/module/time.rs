//! Time module — displays the current local time.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

/// Displays the current local time in `HH:MM:SS` or `HH:MM` format.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct TimeModule {
    time_fn: fn() -> Option<(u8, u8, u8)>,
    show_seconds: bool,
}

impl Default for TimeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeModule {
    /// Creates a new `TimeModule` using the system clock (default `HH:MM:SS`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            time_fn: system_local_time,
            show_seconds: true,
        }
    }

    /// Creates a new `TimeModule` with configurable seconds display.
    #[must_use]
    pub fn with_show_seconds(show_seconds: bool) -> Self {
        Self {
            time_fn: system_local_time,
            show_seconds,
        }
    }

    #[cfg(test)]
    fn with_time_fn(time_fn: fn() -> Option<(u8, u8, u8)>) -> Self {
        Self {
            time_fn,
            show_seconds: true,
        }
    }

    #[cfg(test)]
    fn with_time_fn_and_format(time_fn: fn() -> Option<(u8, u8, u8)>, show_seconds: bool) -> Self {
        Self {
            time_fn,
            show_seconds,
        }
    }
}

impl Module for TimeModule {
    fn name(&self) -> &'static str {
        "time"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, _ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let (hour, minute, second) = (self.time_fn)()?;
        let content = if self.show_seconds {
            format!("{hour:02}:{minute:02}:{second:02}")
        } else {
            format!("{hour:02}:{minute:02}")
        };
        Some(ModuleOutput { content })
    }
}

fn system_local_time() -> Option<(u8, u8, u8)> {
    let now = ::time::OffsetDateTime::now_local().ok()?;
    Some((now.hour(), now.minute(), now.second()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn make_ctx() -> RenderContext<'static> {
        RenderContext {
            cwd: Path::new("/tmp"),
            home_dir: Path::new("/Users/testuser"),
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main",
            cols: 80,
        }
    }

    #[test]
    #[allow(clippy::unnecessary_wraps)]
    fn test_module_formats_time() {
        fn fixed() -> Option<(u8, u8, u8)> {
            Some((14, 5, 9))
        }
        let module = TimeModule::with_time_fn(fixed);
        let ctx = make_ctx();
        let output = module.render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("14:05:09".to_owned()));
    }

    #[test]
    #[allow(clippy::unnecessary_wraps)]
    fn test_module_midnight() {
        fn midnight() -> Option<(u8, u8, u8)> {
            Some((0, 0, 0))
        }
        let module = TimeModule::with_time_fn(midnight);
        let ctx = make_ctx();
        let output = module.render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("00:00:00".to_owned()));
    }

    #[test]
    fn test_module_time_unavailable_returns_none() {
        fn unavailable() -> Option<(u8, u8, u8)> {
            None
        }
        let module = TimeModule::with_time_fn(unavailable);
        let ctx = make_ctx();
        assert!(module.render(&ctx).is_none());
    }

    #[test]
    #[allow(clippy::unnecessary_wraps)]
    fn test_module_formats_time_without_seconds() {
        fn fixed() -> Option<(u8, u8, u8)> {
            Some((14, 5, 9))
        }
        let module = TimeModule::with_time_fn_and_format(fixed, false);
        let ctx = make_ctx();
        let output = module.render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("14:05".to_owned()));
    }
}
