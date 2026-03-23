//! Time module — displays the current local time.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::sealed;

/// Displays the current local time in `HH:MM:SS` or `HH:MM` format.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct TimeModule {
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
    pub const fn new() -> Self {
        Self { show_seconds: true }
    }

    /// Creates a new `TimeModule` with configurable seconds display.
    #[must_use]
    pub const fn with_show_seconds(show_seconds: bool) -> Self {
        Self { show_seconds }
    }
}

impl sealed::Sealed for TimeModule {}

impl Module for TimeModule {
    fn name(&self) -> &'static str {
        "time"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, _ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        render_time(system_local_time(), self.show_seconds)
    }
}

fn format_time(hour: u8, minute: u8, second: u8, show_seconds: bool) -> String {
    if show_seconds {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

fn render_time(time: Option<(u8, u8, u8)>, show_seconds: bool) -> Option<ModuleOutput> {
    let (hour, minute, second) = time?;
    Some(ModuleOutput {
        content: format_time(hour, minute, second, show_seconds),
    })
}

fn system_local_time() -> Option<(u8, u8, u8)> {
    let now = ::time::OffsetDateTime::now_local().ok()?;
    Some((now.hour(), now.minute(), now.second()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_with_seconds() {
        assert_eq!(format_time(14, 5, 9, true), "14:05:09");
    }

    #[test]
    fn format_without_seconds() {
        assert_eq!(format_time(14, 5, 9, false), "14:05");
    }

    #[test]
    fn render_none() {
        assert!(render_time(None, true).is_none());
    }

    #[test]
    fn render_with_seconds() {
        let output = render_time(Some((14, 5, 9)), true);
        assert_eq!(output.map(|o| o.content), Some("14:05:09".to_owned()));
    }

    #[test]
    fn render_without_seconds() {
        let output = render_time(Some((14, 5, 9)), false);
        assert_eq!(output.map(|o| o.content), Some("14:05".to_owned()));
    }
}
