//! Toolchain module — displays the detected language toolchain.

use std::path::Path;

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};

/// File-to-toolchain mappings, checked in order.
const TOOLCHAIN_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", "rust"),
    ("bun.lockb", "bun"),
    ("bunfig.toml", "bun"),
    ("package.json", "node"),
    ("go.mod", "go"),
    ("pyproject.toml", "python"),
    ("setup.py", "python"),
    ("Gemfile", "ruby"),
    ("mix.exs", "elixir"),
];

/// Detects the language toolchain based on marker files in the working directory.
///
/// Returns `None` when no recognized marker file is found.
#[derive(Debug, Default)]
#[allow(clippy::module_name_repetitions)]
pub struct ToolchainModule;

impl ToolchainModule {
    /// Creates a new `ToolchainModule`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Module for ToolchainModule {
    fn name(&self) -> &'static str {
        "toolchain"
    }

    fn speed(&self) -> ModuleSpeed {
        ModuleSpeed::Fast
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let name = detect_toolchain(ctx.cwd)?;
        Some(ModuleOutput {
            content: name.to_owned(),
        })
    }
}

fn detect_toolchain(cwd: &Path) -> Option<&'static str> {
    for &(file, name) in TOOLCHAIN_FILES {
        if cwd.join(file).is_file() {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn make_ctx(cwd: &Path) -> RenderContext<'_> {
        RenderContext {
            cwd,
            home_dir: Path::new("/Users/testuser"),
            last_exit_code: 0,
            duration_ms: None,
            keymap: "main",
            cols: 80,
        }
    }

    #[test]
    fn test_module_detects_rust() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        let content = output.map(|o| o.content);
        assert!(
            content.as_deref().is_some_and(|c| c.contains("rust")),
            "expected 'rust' in output, got: {content:?}"
        );
        Ok(())
    }

    #[test]
    fn test_module_detects_node() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("node".to_owned()));
        Ok(())
    }

    #[test]
    fn test_module_detects_go() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("go.mod"), "module example.com/foo")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("go".to_owned()));
        Ok(())
    }

    #[test]
    fn test_module_no_marker_returns_none() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ctx = make_ctx(dir.path());
        assert!(ToolchainModule::new().render(&ctx).is_none());
        Ok(())
    }

    #[test]
    fn test_module_first_match_wins() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        std::fs::write(dir.path().join("package.json"), "")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        assert_eq!(
            output.map(|o| o.content),
            Some("rust".to_owned()),
            "Cargo.toml should take precedence"
        );
        Ok(())
    }

    #[test]
    fn test_module_detects_bun_lockb() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bun.lockb"), "")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("bun".to_owned()));
        Ok(())
    }

    #[test]
    fn test_module_detects_bunfig_toml() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bunfig.toml"), "")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        assert_eq!(output.map(|o| o.content), Some("bun".to_owned()));
        Ok(())
    }

    #[test]
    fn test_module_bun_takes_precedence_over_node() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bun.lockb"), "")?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        let ctx = make_ctx(dir.path());
        let output = ToolchainModule::new().render(&ctx);
        assert_eq!(
            output.map(|o| o.content),
            Some("bun".to_owned()),
            "bun.lockb should take precedence over package.json"
        );
        Ok(())
    }
}
