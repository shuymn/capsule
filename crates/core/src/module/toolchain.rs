//! Toolchain module — displays detected language toolchains with versions.
//!
//! Detection is data-driven: each [`ResolvedToolchain`] defines marker files,
//! a version command, a regex for extraction, and display metadata (icon, style).
//! Built-in definitions cover 6 toolchains; users add or override via `[[toolchain]]`
//! in config.

use std::{path::Path, process::Command};

use regex_lite::Regex;

use super::{Module, ModuleOutput, ModuleSpeed, RenderContext};
use crate::{
    config::{RegexPattern, ToolchainDef},
    render::style::{Color, Style},
};

/// A compiled toolchain definition ready for detection.
///
/// Created once at daemon startup from built-in + user-defined [`ToolchainDef`]s.
#[derive(Debug, Clone)]
pub struct ResolvedToolchain {
    /// Toolchain identifier (e.g. `"rust"`, `"zig"`).
    pub name: String,
    /// Marker files whose presence triggers detection.
    pub files: Vec<String>,
    /// Optional file in `cwd` containing a version string.
    pub version_file: Option<String>,
    /// Command + args to run for version detection.
    pub command: Option<Vec<String>>,
    /// Compiled regex; first capture group is the version.
    pub version_regex: Option<Regex>,
    /// Nerd Font icon glyph.
    pub icon: Option<String>,
    /// Display style (color + bold).
    pub style: Style,
}

/// Detected toolchain with resolved version and display metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainInfo {
    /// Toolchain identifier (e.g. `"rust"`, `"node"`).
    pub name: String,
    /// Resolved version string with `v` prefix (e.g. `"v1.82.0"`).
    pub version: String,
    /// Nerd Font icon glyph.
    pub icon: Option<String>,
    /// Display style for this toolchain.
    pub style: Style,
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
        let defs = resolve_toolchains(&[]);
        let infos = detect_all(&defs, ctx.cwd, None);
        let first = infos.into_iter().next()?;
        Some(ModuleOutput {
            content: first.version,
        })
    }
}

// ---------------------------------------------------------------------------
// Built-in toolchain definitions
// ---------------------------------------------------------------------------

/// Returns the 6 built-in toolchain definitions in display order.
#[must_use]
pub fn builtin_toolchains() -> Vec<ToolchainDef> {
    vec![
        ToolchainDef {
            name: "rust".to_owned(),
            files: vec!["Cargo.toml".to_owned()],
            version_file: None,
            command: Some(vec!["rustc".to_owned(), "--version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"rustc\s+(\S+)".to_owned())),
            icon: Some("\u{f1617}".to_owned()),
            color: Some(Color::Red),
        },
        ToolchainDef {
            name: "bun".to_owned(),
            files: vec!["bun.lockb".to_owned(), "bunfig.toml".to_owned()],
            version_file: None,
            command: Some(vec!["bun".to_owned(), "--version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"(\d[\d.]*)".to_owned())),
            icon: Some("\u{e76f}".to_owned()),
            color: Some(Color::Red),
        },
        ToolchainDef {
            name: "node".to_owned(),
            files: vec!["package.json".to_owned()],
            version_file: Some(".node-version".to_owned()),
            command: Some(vec!["node".to_owned(), "--version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"v?(\d[\d.]*)".to_owned())),
            icon: Some("\u{e718}".to_owned()),
            color: Some(Color::Green),
        },
        ToolchainDef {
            name: "go".to_owned(),
            files: vec!["go.mod".to_owned()],
            version_file: None,
            command: Some(vec!["go".to_owned(), "version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"go(\d[\d.]*)".to_owned())),
            icon: Some("\u{e627}".to_owned()),
            color: Some(Color::Cyan),
        },
        ToolchainDef {
            name: "python".to_owned(),
            files: vec!["pyproject.toml".to_owned(), "setup.py".to_owned()],
            version_file: None,
            command: Some(vec!["python3".to_owned(), "--version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"Python\s+(\S+)".to_owned())),
            icon: Some("\u{e235}".to_owned()),
            color: Some(Color::Yellow),
        },
        ToolchainDef {
            name: "ruby".to_owned(),
            files: vec!["Gemfile".to_owned()],
            version_file: None,
            command: Some(vec!["ruby".to_owned(), "--version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(
                r"ruby\s+(\d+\.\d+\.\d+)".to_owned(),
            )),
            icon: Some("\u{e791}".to_owned()),
            color: Some(Color::Red),
        },
    ]
}

// ---------------------------------------------------------------------------
// Merge + resolve
// ---------------------------------------------------------------------------

/// Merges built-in toolchain definitions with user-defined ones and compiles regexes.
///
/// User-defined entries with the same `name` as a built-in replace it in-place
/// (preserving order). New entries are appended. The resulting order is deterministic:
/// built-in order first, then user-defined additions.
#[must_use]
pub fn resolve_toolchains(user_defs: &[ToolchainDef]) -> Vec<ResolvedToolchain> {
    let mut defs = builtin_toolchains();
    for ud in user_defs {
        if let Some(existing) = defs.iter_mut().find(|d| d.name == ud.name) {
            *existing = ud.clone();
        } else {
            defs.push(ud.clone());
        }
    }
    defs.into_iter().map(compile_def).collect()
}

fn compile_def(def: ToolchainDef) -> ResolvedToolchain {
    let version_regex = def
        .version_regex
        .as_ref()
        .and_then(|pat| Regex::new(pat.as_str()).ok());
    let style = def.color.map_or_else(
        || Style::new().fg(Color::BrightBlack),
        |c| Style::new().fg(c).bold(),
    );
    ResolvedToolchain {
        name: def.name,
        files: def.files,
        version_file: def.version_file,
        command: def.command,
        version_regex,
        icon: def.icon,
        style,
    }
}

// ---------------------------------------------------------------------------
// Detection (multi-toolchain, data-driven)
// ---------------------------------------------------------------------------

/// Detects all matching toolchains for the given directory.
///
/// Iterates through `defs` in order, checking marker files and resolving versions.
/// Each toolchain name appears at most once (first match wins for duplicate names).
/// Order is deterministic: follows definition order.
///
/// `path_env` overrides the `PATH` environment variable for version
/// detection commands (important under launchd).
#[must_use]
pub fn detect_all(
    defs: &[ResolvedToolchain],
    cwd: &Path,
    path_env: Option<&str>,
) -> Vec<ToolchainInfo> {
    let mut results = Vec::with_capacity(defs.len());

    for def in defs {
        // Skip if we already matched a toolchain with this name (from a previous def
        // with overlapping files — shouldn't happen after merge, but defensive).
        if results.iter().any(|r: &ToolchainInfo| r.name == def.name) {
            continue;
        }

        let file_match = def.files.iter().any(|f| cwd.join(f).is_file());
        if !file_match {
            continue;
        }

        if let Some(version) = detect_version_from_def(def, cwd, path_env) {
            results.push(ToolchainInfo {
                name: def.name.clone(),
                version,
                icon: def.icon.clone(),
                style: def.style,
            });
        }
    }

    results
}

fn detect_version_from_def(
    def: &ResolvedToolchain,
    cwd: &Path,
    path_env: Option<&str>,
) -> Option<String> {
    // Try version_file first
    if let Some(ref vf) = def.version_file
        && let Some(v) = read_version_file(&cwd.join(vf))
    {
        return Some(v);
    }

    // Fall back to command + regex
    let cmd_parts = def.command.as_ref()?;
    if cmd_parts.is_empty() {
        return None;
    }
    let (program, args) = cmd_parts.split_first()?;
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();

    if let Some(ref re) = def.version_regex {
        let caps = re.captures(trimmed)?;
        let ver = caps.get(1)?.as_str();
        Some(ensure_v_prefix(ver))
    } else {
        // No regex: use entire trimmed output
        if trimmed.is_empty() {
            None
        } else {
            Some(ensure_v_prefix(trimmed))
        }
    }
}

/// Reads a version file, returning the trimmed content with `v` prefix.
///
/// Returns `None` if the file doesn't exist, is empty, or contains `/`
/// (filters named versions like `lts/iron`).
fn read_version_file(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        return None;
    }
    Some(ensure_v_prefix(trimmed))
}

fn ensure_v_prefix(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    // -- builtin_toolchains ---------------------------------------------------

    #[test]
    fn test_builtin_toolchains_count() {
        assert_eq!(builtin_toolchains().len(), 6);
    }

    #[test]
    fn test_builtin_toolchains_names() {
        let defs = builtin_toolchains();
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["rust", "bun", "node", "go", "python", "ruby"]);
    }

    // -- resolve_toolchains ---------------------------------------------------

    #[test]
    fn test_resolve_empty_user_defs_returns_builtins() {
        let resolved = resolve_toolchains(&[]);
        assert_eq!(resolved.len(), 6);
        assert_eq!(resolved[0].name, "rust");
    }

    #[test]
    fn test_resolve_user_override_replaces_in_place() {
        let user = vec![ToolchainDef {
            name: "rust".to_owned(),
            files: vec!["Cargo.toml".to_owned()],
            version_file: None,
            command: Some(vec!["rustc".to_owned(), "--version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"rustc\s+(\S+)".to_owned())),
            icon: Some("R".to_owned()),
            color: Some(Color::Blue),
        }];
        let resolved = resolve_toolchains(&user);
        assert_eq!(resolved.len(), 6, "count unchanged");
        assert_eq!(resolved[0].name, "rust", "still first");
        assert_eq!(resolved[0].icon.as_deref(), Some("R"), "icon overridden");
        assert_eq!(resolved[0].style, Style::new().fg(Color::Blue).bold());
    }

    #[test]
    fn test_resolve_user_adds_new_toolchain() {
        let user = vec![ToolchainDef {
            name: "zig".to_owned(),
            files: vec!["build.zig".to_owned()],
            version_file: None,
            command: Some(vec!["zig".to_owned(), "version".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"(\d[\d.]*)".to_owned())),
            icon: Some("Z".to_owned()),
            color: Some(Color::Yellow),
        }];
        let resolved = resolve_toolchains(&user);
        assert_eq!(resolved.len(), 7);
        assert_eq!(resolved[6].name, "zig");
    }

    #[test]
    fn test_resolve_invalid_regex_compiles_to_none() {
        // new_unchecked bypasses validation; compile_def still handles gracefully
        let user = vec![ToolchainDef {
            name: "bad".to_owned(),
            files: vec!["bad.txt".to_owned()],
            version_file: None,
            command: Some(vec!["echo".to_owned(), "1.0".to_owned()]),
            version_regex: Some(RegexPattern::new_unchecked(r"(unclosed".to_owned())),
            icon: None,
            color: None,
        }];
        let resolved = resolve_toolchains(&user);
        let bad = resolved.iter().find(|r| r.name == "bad");
        assert!(
            bad.is_some_and(|b| b.version_regex.is_none()),
            "invalid regex should compile to None"
        );
    }

    #[test]
    fn test_resolve_no_color_defaults_to_bright_black() {
        let user = vec![ToolchainDef {
            name: "test".to_owned(),
            files: vec!["test.txt".to_owned()],
            version_file: None,
            command: None,
            version_regex: None,
            icon: None,
            color: None,
        }];
        let resolved = resolve_toolchains(&user);
        let tc = resolved.iter().find(|r| r.name == "test");
        assert!(tc.is_some());
        assert_eq!(
            tc.map(|t| t.style),
            Some(Style::new().fg(Color::BrightBlack))
        );
    }

    // -- detect_all (file detection) ------------------------------------------

    #[test]
    fn test_detect_all_rust() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        let defs = resolve_toolchains(&[]);
        let results = detect_all(&defs, dir.path(), None);
        // Version detection will fail (no rustc in test env), but file detection works.
        // This test verifies that only marker-file-matched toolchains attempt detection.
        // In CI rustc is available, so this may succeed.
        // We just verify no panic and result is at most 1 rust entry.
        assert!(results.len() <= 1);
        if let Some(tc) = results.first() {
            assert_eq!(tc.name, "rust");
        }
        Ok(())
    }

    #[test]
    fn test_detect_all_no_marker_returns_empty() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let defs = resolve_toolchains(&[]);
        let results = detect_all(&defs, dir.path(), None);
        assert!(results.is_empty());
        Ok(())
    }

    #[test]
    fn test_detect_all_multiple_toolchains() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        let defs = resolve_toolchains(&[]);
        let results = detect_all(&defs, dir.path(), None);
        // Both rust and node markers exist; both may be detected (version may fail)
        let names: Vec<_> = results.iter().map(|r| r.name.as_str()).collect();
        // Order must follow definition order: rust before node
        if names.len() >= 2 {
            let rust_pos = names.iter().position(|n| *n == "rust");
            let node_pos = names.iter().position(|n| *n == "node");
            if let (Some(r), Some(n)) = (rust_pos, node_pos) {
                assert!(r < n, "rust should come before node");
            }
        }
        Ok(())
    }

    #[test]
    fn test_detect_all_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;
        std::fs::write(dir.path().join("go.mod"), "module test")?;
        let defs = resolve_toolchains(&[]);
        let results1 = detect_all(&defs, dir.path(), None);
        let results2 = detect_all(&defs, dir.path(), None);
        let names1: Vec<_> = results1.iter().map(|r| r.name.as_str()).collect();
        let names2: Vec<_> = results2.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names1, names2, "order must be deterministic across calls");
        Ok(())
    }

    #[test]
    fn test_detect_all_elixir_removed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("mix.exs"), "")?;
        let defs = resolve_toolchains(&[]);
        let results = detect_all(&defs, dir.path(), None);
        assert!(results.is_empty(), "Elixir should no longer be detected");
        Ok(())
    }

    #[test]
    fn test_detect_all_bun_takes_precedence_file_order() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("bun.lockb"), "")?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        let defs = resolve_toolchains(&[]);
        let results = detect_all(&defs, dir.path(), None);
        // Both bun and node may be detected, but bun should come first
        let names: Vec<_> = results.iter().map(|r| r.name.as_str()).collect();
        if names.len() >= 2 {
            let bun_pos = names.iter().position(|n| *n == "bun");
            let node_pos = names.iter().position(|n| *n == "node");
            if let (Some(b), Some(n)) = (bun_pos, node_pos) {
                assert!(b < n, "bun should come before node");
            }
        }
        Ok(())
    }

    // -- detect_version_from_def (regex-based parsing) ------------------------

    fn make_echo_def(name: &str, output: &str, regex: &str) -> ResolvedToolchain {
        ResolvedToolchain {
            name: name.to_owned(),
            files: vec![],
            version_file: None,
            command: Some(vec!["echo".to_owned(), output.to_owned()]),
            version_regex: Regex::new(regex).ok(),
            icon: None,
            style: Style::default(),
        }
    }

    #[test]
    fn test_version_regex_rust() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def(
            "rust",
            "rustc 1.82.0 (f6e511eec 2024-10-15)",
            r"rustc\s+(\S+)",
        );
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v1.82.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_rust_nightly() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def(
            "rust",
            "rustc 1.83.0-nightly (aedd173a2 2024-11-01)",
            r"rustc\s+(\S+)",
        );
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v1.83.0-nightly"));
        Ok(())
    }

    #[test]
    fn test_version_regex_node() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def("node", "v22.0.0", r"v?(\d[\d.]*)");
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v22.0.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_node_without_v() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def("node", "22.0.0", r"v?(\d[\d.]*)");
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v22.0.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_go() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def("go", "go version go1.22.0 darwin/arm64", r"go(\d[\d.]*)");
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v1.22.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_python() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def("python", "Python 3.12.0", r"Python\s+(\S+)");
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v3.12.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_ruby() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def(
            "ruby",
            "ruby 3.3.0p0 (2023-12-25 revision 5124f9ac75) [arm64-darwin23]",
            r"ruby\s+(\d+\.\d+\.\d+)",
        );
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v3.3.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_ruby_without_patch() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def(
            "ruby",
            "ruby 3.3.0 (2023-12-25 revision 5124f9ac75) [arm64-darwin23]",
            r"ruby\s+(\d+\.\d+\.\d+)",
        );
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v3.3.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_bun() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def("bun", "1.1.0", r"(\d[\d.]*)");
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v1.1.0"));
        Ok(())
    }

    #[test]
    fn test_version_regex_custom_zig() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = make_echo_def("zig", "0.13.0", r"(\d[\d.]*)");
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v0.13.0"));
        Ok(())
    }

    #[test]
    fn test_version_no_regex_uses_full_output() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = ResolvedToolchain {
            name: "test".to_owned(),
            files: vec![],
            version_file: None,
            command: Some(vec!["echo".to_owned(), "1.2.3".to_owned()]),
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v1.2.3"));
        Ok(())
    }

    #[test]
    fn test_version_no_command_returns_none() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = ResolvedToolchain {
            name: "test".to_owned(),
            files: vec![],
            version_file: None,
            command: None,
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version, None);
        Ok(())
    }

    #[test]
    fn test_version_command_not_found_returns_none() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = ResolvedToolchain {
            name: "test".to_owned(),
            files: vec![],
            version_file: None,
            command: Some(vec!["nonexistent_binary_xyz".to_owned()]),
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version, None);
        Ok(())
    }

    // -- version_file ---------------------------------------------------------

    #[test]
    fn test_version_file_plain() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "22.0.0\n")?;
        let def = ResolvedToolchain {
            name: "node".to_owned(),
            files: vec![],
            version_file: Some(".node-version".to_owned()),
            command: None,
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v22.0.0"));
        Ok(())
    }

    #[test]
    fn test_version_file_with_v_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "v22.0.0\n")?;
        let def = ResolvedToolchain {
            name: "node".to_owned(),
            files: vec![],
            version_file: Some(".node-version".to_owned()),
            command: None,
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version.as_deref(), Some("v22.0.0"));
        Ok(())
    }

    #[test]
    fn test_version_file_named_version_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "lts/iron\n")?;
        let def = ResolvedToolchain {
            name: "node".to_owned(),
            files: vec![],
            version_file: Some(".node-version".to_owned()),
            command: None,
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version, None, "named versions should be ignored");
        Ok(())
    }

    #[test]
    fn test_version_file_empty() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".node-version"), "\n")?;
        let def = ResolvedToolchain {
            name: "node".to_owned(),
            files: vec![],
            version_file: Some(".node-version".to_owned()),
            command: None,
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version, None);
        Ok(())
    }

    #[test]
    fn test_version_file_missing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let def = ResolvedToolchain {
            name: "node".to_owned(),
            files: vec![],
            version_file: Some(".node-version".to_owned()),
            command: None,
            version_regex: None,
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(version, None);
        Ok(())
    }

    #[test]
    fn test_version_file_takes_precedence_over_command() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        // Write a version file that returns 20.0.0
        std::fs::write(dir.path().join(".node-version"), "20.0.0\n")?;
        let def = ResolvedToolchain {
            name: "node".to_owned(),
            files: vec![],
            version_file: Some(".node-version".to_owned()),
            // Command would return 22.0.0 if called
            command: Some(vec!["echo".to_owned(), "v22.0.0".to_owned()]),
            version_regex: Some(Regex::new(r"v?(\d[\d.]*)").ok()).flatten(),
            icon: None,
            style: Style::default(),
        };
        let version = detect_version_from_def(&def, dir.path(), None);
        assert_eq!(
            version.as_deref(),
            Some("v20.0.0"),
            "version_file should take precedence"
        );
        Ok(())
    }

    // -- ToolchainInfo carries display metadata -------------------------------

    #[test]
    fn test_detect_all_carries_icon_and_style() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker.txt"), "")?;
        let defs = vec![ResolvedToolchain {
            name: "test".to_owned(),
            files: vec!["marker.txt".to_owned()],
            version_file: None,
            command: Some(vec!["echo".to_owned(), "1.0.0".to_owned()]),
            version_regex: Some(Regex::new(r"(\d[\d.]*)").ok()).flatten(),
            icon: Some("T".to_owned()),
            style: Style::new().fg(Color::Magenta).bold(),
        }];
        let results = detect_all(&defs, dir.path(), None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].icon.as_deref(), Some("T"));
        assert_eq!(results[0].style, Style::new().fg(Color::Magenta).bold());
        assert_eq!(results[0].version, "v1.0.0");
        Ok(())
    }

    // -- Module trait -----------------------------------------------------------

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
