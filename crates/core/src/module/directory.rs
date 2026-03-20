//! Directory module — displays the current working directory.

use std::path::Path;

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

/// Displays the current working directory, substituting `~` for the home directory.
#[derive(Debug, Default)]
#[allow(clippy::module_name_repetitions)]
pub struct DirectoryModule;

impl DirectoryModule {
    /// Creates a new `DirectoryModule`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Module for DirectoryModule {
    fn name(&self) -> &'static str {
        "directory"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let content = abbreviate_home(ctx.cwd, ctx.home_dir);
        Some(ModuleOutput { content })
    }
}

fn abbreviate_home(cwd: &Path, home: &Path) -> String {
    if cwd == home {
        return "~".to_owned();
    }
    if let Ok(suffix) = cwd.strip_prefix(home) {
        let lossy = suffix.to_string_lossy();
        let mut result = String::with_capacity(lossy.len() + 2);
        result.push('~');
        result.push('/');
        result.push_str(&lossy);
        return result;
    }
    cwd.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx<'a>(cwd: &'a Path, home: &'a Path) -> RenderContext<'a> {
        RenderContext {
            cwd,
            home_dir: home,
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main",
            cols: 80,
        }
    }

    #[test]
    fn test_module_cwd_is_home() {
        let home = Path::new("/Users/testuser");
        let ctx = make_ctx(home, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("~".to_owned()));
    }

    #[test]
    fn test_module_cwd_under_home() {
        let home = Path::new("/Users/testuser");
        let cwd = Path::new("/Users/testuser/projects/capsule");
        let ctx = make_ctx(cwd, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(
            output.map(|o| o.content),
            Some("~/projects/capsule".to_owned())
        );
    }

    #[test]
    fn test_module_cwd_outside_home() {
        let home = Path::new("/Users/testuser");
        let cwd = Path::new("/tmp");
        let ctx = make_ctx(cwd, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("/tmp".to_owned()));
    }

    #[test]
    fn test_module_cwd_is_root() {
        let home = Path::new("/Users/testuser");
        let cwd = Path::new("/");
        let ctx = make_ctx(cwd, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("/".to_owned()));
    }

    #[test]
    fn test_module_speed_is_fast() {
        assert_eq!(DirectoryModule::new().speed(), ModuleSpeed::Fast);
    }

    #[test]
    fn test_module_name() {
        assert_eq!(DirectoryModule::new().name(), "directory");
    }
}
