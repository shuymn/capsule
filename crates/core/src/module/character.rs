//! Character module — displays the prompt character.

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::sealed;

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

impl sealed::Sealed for CharacterModule {}

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
}
