//! Directory module — displays the current working directory.

use std::path::Path;

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::sealed;

/// Displays the current working directory.
///
/// In a git repo: repo-relative path (folder name at root).
/// Outside git: home-abbreviated path (`~/...`).
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

impl sealed::Sealed for DirectoryModule {}

impl Module for DirectoryModule {
    fn name(&self) -> &'static str {
        "directory"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let content = format_directory(ctx.cwd, ctx.home_dir);
        Some(ModuleOutput { content })
    }
}

fn format_directory(cwd: &Path, home: &Path) -> String {
    if let Some(repo_root) = find_git_root(cwd) {
        if cwd == repo_root {
            // At repo root: show folder name
            return repo_root.file_name().map_or_else(
                || abbreviate_home(cwd, home),
                |n| n.to_string_lossy().into_owned(),
            );
        }
        // Inside repo: show repo-relative path
        if let Ok(relative) = cwd.strip_prefix(repo_root) {
            return relative.to_string_lossy().into_owned();
        }
    }
    // Not in git repo: home-abbreviated
    abbreviate_home(cwd, home)
}

fn find_git_root(start: &Path) -> Option<&Path> {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        dir = dir.parent()?;
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

    // -- Non-git paths (home abbreviation) --

    #[test]
    fn test_module_cwd_is_home() {
        let home = Path::new("/Users/testuser");
        let ctx = make_ctx(home, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("~".to_owned()));
    }

    #[test]
    fn test_module_cwd_outside_home() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let home = Path::new("/Users/testuser");
        let ctx = make_ctx(dir.path(), home);
        let output = DirectoryModule::new().render(&ctx);
        // No .git above, so home-abbreviated
        let content = output.map(|o| o.content);
        assert!(
            content.as_ref().is_some_and(|c| !c.is_empty()),
            "should produce output: {content:?}"
        );
        Ok(())
    }

    #[test]
    fn test_module_cwd_is_root() {
        let home = Path::new("/Users/testuser");
        let cwd = Path::new("/");
        let ctx = make_ctx(cwd, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("/".to_owned()));
    }

    // -- Git repo paths --

    #[test]
    fn test_module_at_git_repo_root() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        let home = Path::new("/Users/testuser");
        let ctx = make_ctx(dir.path(), home);
        let output = DirectoryModule::new().render(&ctx);
        let content = output.map(|o| o.content);
        // Should show folder name, not full path
        let folder_name = dir
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        assert_eq!(content, folder_name);
        Ok(())
    }

    #[test]
    fn test_module_inside_git_repo() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join(".git"))?;
        let sub = dir.path().join("src").join("module");
        std::fs::create_dir_all(&sub)?;
        let home = Path::new("/Users/testuser");
        let ctx = make_ctx(&sub, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(
            output.map(|o| o.content),
            Some("src/module".to_owned()),
            "should show repo-relative path"
        );
        Ok(())
    }

    #[test]
    fn test_module_cwd_under_home_no_git() {
        // Use a path that does not exist on disk (no .git can be found)
        let home = Path::new("/Users/testuser");
        let cwd = Path::new("/Users/testuser/nonexistent/projects/capsule");
        let ctx = make_ctx(cwd, home);
        let output = DirectoryModule::new().render(&ctx);
        assert_eq!(
            output.map(|o| o.content),
            Some("~/nonexistent/projects/capsule".to_owned()),
            "should fall back to home-abbreviated when no .git"
        );
    }
}
