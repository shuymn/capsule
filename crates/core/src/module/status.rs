//! Status module — displays the exit code of the last command.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

/// Displays the exit code when the last command failed (non-zero exit).
///
/// Returns `None` when the exit code is zero.
#[derive(Debug, Default)]
#[allow(clippy::module_name_repetitions)]
pub struct StatusModule;

impl StatusModule {
    /// Creates a new `StatusModule`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Module for StatusModule {
    fn name(&self) -> &'static str {
        "status"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        if ctx.last_exit_code == 0 {
            return None;
        }
        Some(ModuleOutput {
            content: ctx.last_exit_code.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn make_ctx(exit_code: i32) -> RenderContext<'static> {
        RenderContext {
            cwd: Path::new("/tmp"),
            home_dir: Path::new("/Users/testuser"),
            last_exit_code: exit_code,
            duration_ms: None,
            keymap: "main",
            cols: 80,
        }
    }

    #[test]
    fn test_module_exit_code_zero_returns_none() {
        let ctx = make_ctx(0);
        assert!(StatusModule::new().render(&ctx).is_none());
    }

    #[test]
    fn test_module_exit_code_one() {
        let ctx = make_ctx(1);
        let output = StatusModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("1".to_owned()));
    }

    #[test]
    fn test_module_exit_code_130() {
        let ctx = make_ctx(130);
        let output = StatusModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("130".to_owned()));
    }

    #[test]
    fn test_module_negative_exit_code() {
        let ctx = make_ctx(-1);
        let output = StatusModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("-1".to_owned()));
    }
}
