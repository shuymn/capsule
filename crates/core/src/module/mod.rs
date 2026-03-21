//! Module system for the capsule prompt engine.
//!
//! Each module produces a segment of the prompt based on the current
//! [`RenderContext`]. Modules are classified as [`Fast`](ModuleSpeed::Fast)
//! or [`Slow`](ModuleSpeed::Slow) so the daemon can schedule them appropriately.

pub mod character;
pub mod cmd_duration;
pub mod custom;
pub mod directory;
pub mod git;
pub mod status;
pub mod time;

use std::path::Path;

pub use character::CharacterModule;
pub use cmd_duration::CmdDurationModule;
pub use custom::{
    CustomModuleInfo, ResolvedModule, detect_modules, required_env_var_names, resolve_modules,
};
pub use directory::DirectoryModule;
pub use git::{CommandGitProvider, GitError, GitModule, GitProvider, GitStatus};
pub use status::StatusModule;
pub use time::TimeModule;

/// Speed classification for prompt modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleSpeed {
    /// Completes quickly with minimal or no I/O.
    Fast,
    /// May take significant time (e.g., spawning external processes).
    Slow,
}

/// Input context provided to modules for rendering.
#[derive(Debug)]
pub struct RenderContext<'a> {
    /// Current working directory.
    pub cwd: &'a Path,
    /// User's home directory for tilde substitution.
    pub home_dir: &'a Path,
    /// Exit code of the last command.
    pub last_exit_code: i32,
    /// Duration of the last command in milliseconds.
    pub duration_ms: Option<u64>,
    /// Current zle keymap name.
    pub keymap: &'a str,
    /// Terminal width in columns.
    pub cols: u16,
}

/// Output produced by a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleOutput {
    /// Text content of the module output.
    pub content: String,
}

/// A prompt module that produces a segment of the prompt.
pub trait Module {
    /// Module name for identification.
    fn name(&self) -> &'static str;

    /// Whether this module is fast or slow.
    fn speed(&self) -> ModuleSpeed;

    /// Render the module output, or `None` if the module has nothing to show.
    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput>;
}
