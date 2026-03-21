//! Toolchain module — displays the detected language toolchain with version.

use std::{path::Path, process::Command};

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
];

/// Detected toolchain with resolved version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainInfo {
    /// Toolchain identifier (e.g. `"rust"`, `"node"`).
    pub name: &'static str,
    /// Resolved version string with `v` prefix (e.g. `"v1.82.0"`).
    pub version: String,
}

/// Detects the language toolchain and resolves its version.
///
/// Returns `None` when no recognized marker file is found or when
/// version detection fails.
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
        ModuleSpeed::Slow
    }

    fn render(&self, ctx: &RenderContext<'_>) -> Option<ModuleOutput> {
        let info = detect(ctx.cwd, None)?;
        Some(ModuleOutput {
            content: info.version,
        })
    }
}

/// Detects toolchain and resolves version for the given directory.
///
/// `path_env` overrides the `PATH` environment variable for version
/// detection commands (important under launchd).
///
/// Returns `None` if no marker file is found or version cannot be resolved.
#[must_use]
pub fn detect(cwd: &Path, path_env: Option<&str>) -> Option<ToolchainInfo> {
    let name = detect_toolchain(cwd)?;
    let version = detect_version(name, cwd, path_env)?;
    Some(ToolchainInfo { name, version })
}

fn detect_toolchain(cwd: &Path) -> Option<&'static str> {
    for &(file, name) in TOOLCHAIN_FILES {
        if cwd.join(file).is_file() {
            return Some(name);
        }
    }
    None
}

fn detect_version(name: &str, cwd: &Path, path_env: Option<&str>) -> Option<String> {
    if name == "node"
        && let Some(v) = read_node_version_file(cwd)
    {
        return Some(v);
    }

    let (cmd, args) = version_command(name)?;
    let mut command = Command::new(cmd);
    command.args(args).current_dir(cwd);
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_version(name, stdout.trim())
}

fn version_command(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    match name {
        "rust" => Some(("rustc", &["--version"])),
        "node" => Some(("node", &["--version"])),
        "go" => Some(("go", &["version"])),
        "python" => Some(("python3", &["--version"])),
        "ruby" => Some(("ruby", &["--version"])),
        "bun" => Some(("bun", &["--version"])),
        _ => None,
    }
}

fn ensure_v_prefix(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

fn parse_version(name: &str, output: &str) -> Option<String> {
    match name {
        // "rustc 1.82.0 (...)" / "Python 3.12.0"
        "rust" | "python" => {
            let ver = output.split_whitespace().nth(1)?;
            Some(format!("v{ver}"))
        }
        // "v22.0.0" / "1.1.0"
        "node" | "bun" => Some(ensure_v_prefix(output)),
        // "go version go1.22.0 darwin/arm64"
        "go" => {
            for word in output.split_whitespace() {
                if let Some(ver) = word.strip_prefix("go")
                    && ver.starts_with(|c: char| c.is_ascii_digit())
                {
                    return Some(format!("v{ver}"));
                }
            }
            None
        }
        // "ruby 3.3.0p0 (2023-12-25 ...)" or "ruby 3.3.0 (...)"
        "ruby" => {
            let token = output.split_whitespace().nth(1)?;
            let ver = token.split('p').next()?;
            Some(format!("v{ver}"))
        }
        _ => None,
    }
}

fn read_node_version_file(cwd: &Path) -> Option<String> {
    let content = std::fs::read_to_string(cwd.join(".node-version")).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    Some(ensure_v_prefix(trimmed))
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

    // -- detect_toolchain (name detection, fast) ------------------------------

    #[test]
    fn test_detect_toolchain_rust() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        assert_eq!(detect_toolchain(dir.path()), Some("rust"));
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_node() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        assert_eq!(detect_toolchain(dir.path()), Some("node"));
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_go() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("go.mod"), "module example.com/foo")?;
        assert_eq!(detect_toolchain(dir.path()), Some("go"));
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_no_marker_returns_none() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        assert_eq!(detect_toolchain(dir.path()), None);
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_first_match_wins() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        std::fs::write(dir.path().join("package.json"), "")?;
        assert_eq!(
            detect_toolchain(dir.path()),
            Some("rust"),
            "Cargo.toml should take precedence"
        );
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_bun_lockb() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bun.lockb"), "")?;
        assert_eq!(detect_toolchain(dir.path()), Some("bun"));
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_bunfig_toml() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bunfig.toml"), "")?;
        assert_eq!(detect_toolchain(dir.path()), Some("bun"));
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_bun_takes_precedence_over_node()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bun.lockb"), "")?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        assert_eq!(
            detect_toolchain(dir.path()),
            Some("bun"),
            "bun.lockb should take precedence over package.json"
        );
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_elixir_removed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("mix.exs"), "")?;
        assert_eq!(
            detect_toolchain(dir.path()),
            None,
            "Elixir should no longer be detected"
        );
        Ok(())
    }

    // -- parse_version --------------------------------------------------------

    #[test]
    fn test_parse_version_rust() {
        assert_eq!(
            parse_version("rust", "rustc 1.82.0 (f6e511eec 2024-10-15)"),
            Some("v1.82.0".to_owned())
        );
    }

    #[test]
    fn test_parse_version_rust_nightly() {
        assert_eq!(
            parse_version("rust", "rustc 1.83.0-nightly (aedd173a2 2024-11-01)"),
            Some("v1.83.0-nightly".to_owned())
        );
    }

    #[test]
    fn test_parse_version_node() {
        assert_eq!(parse_version("node", "v22.0.0"), Some("v22.0.0".to_owned()));
    }

    #[test]
    fn test_parse_version_node_without_v_prefix() {
        assert_eq!(parse_version("node", "22.0.0"), Some("v22.0.0".to_owned()));
    }

    #[test]
    fn test_parse_version_go() {
        assert_eq!(
            parse_version("go", "go version go1.22.0 darwin/arm64"),
            Some("v1.22.0".to_owned())
        );
    }

    #[test]
    fn test_parse_version_python() {
        assert_eq!(
            parse_version("python", "Python 3.12.0"),
            Some("v3.12.0".to_owned())
        );
    }

    #[test]
    fn test_parse_version_ruby() {
        assert_eq!(
            parse_version(
                "ruby",
                "ruby 3.3.0p0 (2023-12-25 revision 5124f9ac75) [arm64-darwin23]"
            ),
            Some("v3.3.0".to_owned())
        );
    }

    #[test]
    fn test_parse_version_ruby_without_patch_suffix() {
        assert_eq!(
            parse_version(
                "ruby",
                "ruby 3.3.0 (2023-12-25 revision 5124f9ac75) [arm64-darwin23]"
            ),
            Some("v3.3.0".to_owned())
        );
    }

    #[test]
    fn test_parse_version_bun() {
        assert_eq!(parse_version("bun", "1.1.0"), Some("v1.1.0".to_owned()));
    }

    #[test]
    fn test_parse_version_unknown_toolchain() {
        assert_eq!(parse_version("unknown", "anything"), None);
    }

    // -- read_node_version_file -----------------------------------------------

    #[test]
    fn test_node_version_file_plain_version() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "22.0.0\n")?;
        assert_eq!(
            read_node_version_file(dir.path()),
            Some("v22.0.0".to_owned())
        );
        Ok(())
    }

    #[test]
    fn test_node_version_file_with_v_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "v22.0.0\n")?;
        assert_eq!(
            read_node_version_file(dir.path()),
            Some("v22.0.0".to_owned())
        );
        Ok(())
    }

    #[test]
    fn test_node_version_file_named_version_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "lts/iron\n")?;
        assert_eq!(
            read_node_version_file(dir.path()),
            None,
            "named versions should be ignored"
        );
        Ok(())
    }

    #[test]
    fn test_node_version_file_empty() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "\n")?;
        assert_eq!(read_node_version_file(dir.path()), None);
        Ok(())
    }

    #[test]
    fn test_node_version_file_missing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        assert_eq!(read_node_version_file(dir.path()), None);
        Ok(())
    }

    // -- Module trait ---------------------------------------------------------

    #[test]
    fn test_module_speed_is_slow() {
        assert_eq!(ToolchainModule::new().speed(), ModuleSpeed::Slow);
    }

    #[test]
    fn test_module_no_marker_returns_none() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ctx = make_ctx(dir.path());
        assert!(ToolchainModule::new().render(&ctx).is_none());
        Ok(())
    }
}
