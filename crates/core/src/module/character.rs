//! Character module — displays the prompt character.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

/// Prompt character shown on success.
const SUCCESS_CHAR: &str = "❯";

/// Prompt character shown on failure.
const ERROR_CHAR: &str = "✗";

/// Displays a prompt character that differs based on the last command's exit status.
#[derive(Debug, Default)]
#[allow(clippy::module_name_repetitions)]
pub struct CharacterModule;

impl CharacterModule {
    /// Creates a new `CharacterModule`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Module for CharacterModule {
    fn name(&self) -> &'static str {
        "character"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let ch = if ctx.last_exit_code == 0 {
            SUCCESS_CHAR
        } else {
            ERROR_CHAR
        };
        Some(ModuleOutput {
            content: ch.to_owned(),
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
    fn test_module_success_character() {
        let ctx = make_ctx(0);
        let output = CharacterModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("❯".to_owned()));
    }

    #[test]
    fn test_module_error_character() {
        let ctx = make_ctx(1);
        let output = CharacterModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("✗".to_owned()));
    }

    #[test]
    fn test_module_success_and_error_differ() {
        let success_ctx = make_ctx(0);
        let error_ctx = make_ctx(1);
        let success = CharacterModule::new().render(&success_ctx);
        let error = CharacterModule::new().render(&error_ctx);
        assert_ne!(success, error, "success and error outputs must differ");
    }
}
