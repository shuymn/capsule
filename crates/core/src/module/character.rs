//! Character module — displays the prompt character.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

/// Prompt character (always `❯`; color varies by exit status in composition).
const PROMPT_CHAR: &str = "❯";

/// Displays the prompt character `❯`.
///
/// Always outputs the same character; the composition layer applies
/// green on success and red on error.
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

    fn render(&self, _ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        Some(ModuleOutput {
            content: PROMPT_CHAR.to_owned(),
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
    fn test_module_always_outputs_prompt_char() {
        let ctx = make_ctx(0);
        let output = CharacterModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("❯".to_owned()));
    }

    #[test]
    fn test_module_same_char_on_error() {
        let ctx = make_ctx(1);
        let output = CharacterModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("❯".to_owned()));
    }

    #[test]
    fn test_module_success_and_error_same_content() {
        let success_ctx = make_ctx(0);
        let error_ctx = make_ctx(1);
        let success = CharacterModule::new().render(&success_ctx);
        let error = CharacterModule::new().render(&error_ctx);
        assert_eq!(
            success, error,
            "character content should be the same; color is applied by composition"
        );
    }
}
