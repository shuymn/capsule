//! Configuration file loading and defaults.
//!
//! Reads `$XDG_CONFIG_HOME/capsule/config.toml` (fallback `~/.capsule/config.toml`).
//! Missing file → compiled-in defaults. Parse error → log + defaults.

use std::path::{Path, PathBuf};

use crate::render::style::Color;

/// Top-level configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    /// Character module settings.
    pub character: CharacterConfig,
    /// Directory module settings.
    pub directory: DirectoryConfig,
    /// Git module settings.
    pub git: GitConfig,
    /// Time module settings.
    pub time: TimeConfig,
    /// Command duration module settings.
    pub cmd_duration: CmdDurationConfig,
    /// Connector words between segments.
    pub connectors: ConnectorConfig,
    /// User-defined prompt modules (`[[module]]` array).
    #[serde(default)]
    pub module: Vec<ModuleDef>,
}

/// Character prompt settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct CharacterConfig {
    /// The prompt character glyph.
    pub glyph: String,
    /// Color when last command succeeded.
    pub success_color: Color,
    /// Color when last command failed.
    pub error_color: Color,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            glyph: "\u{276f}".to_owned(),
            success_color: Color::Green,
            error_color: Color::Red,
        }
    }
}

/// Directory module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct DirectoryConfig {
    /// Foreground color for the directory path (bold is always applied).
    pub color: Color,
}

impl Default for DirectoryConfig {
    fn default() -> Self {
        Self { color: Color::Cyan }
    }
}

/// Git module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// Nerd Font icon glyph for git branch.
    pub icon: String,
    /// Color for the bracket indicators (bold is always applied).
    pub indicator_color: Color,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            icon: "\u{f418}".to_owned(),
            indicator_color: Color::Red,
        }
    }
}

/// Supported time display formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    /// `HH:MM:SS` — hours, minutes, seconds.
    WithSeconds,
    /// `HH:MM` — hours and minutes only.
    WithoutSeconds,
}

impl TimeFormat {
    /// Whether seconds should be shown.
    #[must_use]
    pub const fn show_seconds(self) -> bool {
        matches!(self, Self::WithSeconds)
    }
}

impl<'de> serde::Deserialize<'de> for TimeFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = TimeFormat;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"HH:MM:SS\" or \"HH:MM\"")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "HH:MM:SS" => Ok(TimeFormat::WithSeconds),
                    "HH:MM" => Ok(TimeFormat::WithoutSeconds),
                    _ => Err(E::custom(format!(
                        "unsupported time format `{v}`, expected \"HH:MM:SS\" or \"HH:MM\""
                    ))),
                }
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

/// Time module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct TimeConfig {
    /// Whether the time segment is displayed.
    pub enabled: bool,
    /// Time format.
    pub format: TimeFormat,
    /// Foreground color for the time segment.
    pub color: Color,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            format: TimeFormat::WithSeconds,
            color: Color::Yellow,
        }
    }
}

impl TimeConfig {
    /// Whether seconds should be shown in the time output.
    #[must_use]
    pub const fn show_seconds(&self) -> bool {
        self.format.show_seconds()
    }
}

/// Command duration module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct CmdDurationConfig {
    /// Minimum duration in milliseconds before showing the segment.
    pub threshold_ms: u64,
    /// Foreground color for the duration segment.
    pub color: Color,
}

impl Default for CmdDurationConfig {
    fn default() -> Self {
        Self {
            threshold_ms: 2000,
            color: Color::Yellow,
        }
    }
}

/// A regex pattern validated at deserialization time.
#[derive(Debug, Clone)]
pub struct RegexPattern(String);

impl RegexPattern {
    /// Create from a known-valid pattern string (no validation).
    #[must_use]
    pub(crate) const fn new_unchecked(s: String) -> Self {
        Self(s)
    }

    /// Returns the pattern string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> serde::Deserialize<'de> for RegexPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        regex_lite::Regex::new(&s)
            .map_err(|e| serde::de::Error::custom(format!("invalid regex: {e}")))?;
        Ok(Self(s))
    }
}

/// User-defined prompt module entry from `[[module]]` in config.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModuleDef {
    /// Module identifier (e.g. `"aws"`, `"terraform"`).
    pub name: String,
    /// Conditions that trigger this module.
    #[serde(default)]
    pub when: ModuleWhen,
    /// Ordered list of value sources (env, file, command).
    pub source: Vec<SourceDef>,
    /// Format string with `{value}` placeholder.
    #[serde(default = "default_module_format")]
    pub format: String,
    /// Nerd Font icon glyph.
    #[serde(default)]
    pub icon: Option<String>,
    /// Foreground color (bold is always applied).
    #[serde(default)]
    pub color: Option<Color>,
    /// Connector word before this segment.
    #[serde(default)]
    pub connector: Option<String>,
}

fn default_module_format() -> String {
    "{value}".to_owned()
}

/// Conditions that trigger a module.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ModuleWhen {
    /// Marker files whose presence in cwd triggers the module.
    pub files: Vec<String>,
    /// Environment variables whose presence triggers the module.
    pub env: Vec<String>,
}

/// A single value source within a module definition.
///
/// Exactly one of `env`, `file`, or `command` should be set.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SourceDef {
    /// Read value from an environment variable.
    #[serde(default)]
    pub env: Option<String>,
    /// Read value from a file in cwd.
    #[serde(default)]
    pub file: Option<String>,
    /// Run a command and use its stdout.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Regex applied to the source output; first capture group is the value.
    #[serde(default)]
    pub regex: Option<RegexPattern>,
}

impl SourceDef {
    /// Whether this source requires executing an external command.
    #[must_use]
    pub const fn is_command(&self) -> bool {
        self.command.is_some()
    }
}

/// Connector words between prompt segments.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorConfig {
    /// Connector before git segment.
    pub git: String,
    /// Connector before time segment.
    pub time: String,
    /// Connector before `cmd_duration` segment.
    pub cmd_duration: String,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            git: "on".to_owned(),
            time: "at".to_owned(),
            cmd_duration: "took".to_owned(),
        }
    }
}

/// Resolve the config file path.
///
/// Uses `$XDG_CONFIG_HOME/capsule/config.toml` if set, otherwise
/// `~/.capsule/config.toml`.
///
/// Returns `None` if neither `$XDG_CONFIG_HOME` nor `$HOME` is set.
#[must_use]
pub fn resolve_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("capsule/config.toml"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".capsule/config.toml"))
}

/// Load configuration from the given path.
///
/// - If the file does not exist, returns compiled-in defaults.
/// - If the file has syntax errors, logs the error and returns defaults.
pub fn load_config(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(config) => config,
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "config parse error, using defaults");
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "failed to read config, using defaults");
            Config::default()
        }
    }
}

/// Load configuration from the default resolved path, or return defaults.
#[must_use]
pub fn load_default_config() -> Config {
    resolve_config_path().map_or_else(Config::default, |path| load_config(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_matches_hardcoded_values() {
        let config = Config::default();
        assert_eq!(config.character.glyph, "\u{276f}");
        assert_eq!(config.character.success_color, Color::Green);
        assert_eq!(config.character.error_color, Color::Red);
        assert_eq!(config.directory.color, Color::Cyan);
        assert_eq!(config.git.icon, "\u{f418}");
        assert_eq!(config.git.indicator_color, Color::Red);
        assert!(config.time.enabled);
        assert_eq!(config.time.format, TimeFormat::WithSeconds);
        assert_eq!(config.time.color, Color::Yellow);
        assert_eq!(config.cmd_duration.threshold_ms, 2000);
        assert_eq!(config.cmd_duration.color, Color::Yellow);
        assert_eq!(config.connectors.git, "on");
        assert_eq!(config.connectors.time, "at");
        assert_eq!(config.connectors.cmd_duration, "took");
    }

    #[test]
    fn test_config_load_missing_file_returns_defaults() {
        let config = load_config(Path::new("/nonexistent/config.toml"));
        assert_eq!(config.character.glyph, "\u{276f}");
        assert_eq!(config.cmd_duration.threshold_ms, 2000);
    }

    #[test]
    fn test_config_load_empty_file_returns_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "")?;
        let config = load_config(&path);
        assert_eq!(config.character.glyph, "\u{276f}");
        assert_eq!(config.cmd_duration.threshold_ms, 2000);
        Ok(())
    }

    #[test]
    fn test_config_load_partial_overrides() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[character]
glyph = "$"

[cmd_duration]
threshold_ms = 5000
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.character.glyph, "$");
        assert_eq!(config.cmd_duration.threshold_ms, 5000);
        // Non-overridden fields keep defaults
        assert_eq!(config.character.success_color, Color::Green);
        assert_eq!(config.directory.color, Color::Cyan);
        Ok(())
    }

    #[test]
    fn test_config_load_syntax_error_returns_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "invalid = [toml content")?;
        let config = load_config(&path);
        assert_eq!(config.character.glyph, "\u{276f}");
        Ok(())
    }

    #[test]
    fn test_config_time_disabled() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r"
[time]
enabled = false
",
        )?;
        let config = load_config(&path);
        assert!(!config.time.enabled);
        Ok(())
    }

    #[test]
    fn test_config_color_deserialization() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[character]
success_color = "magenta"
error_color = "yellow"

[directory]
color = "blue"
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.character.success_color, Color::Magenta);
        assert_eq!(config.character.error_color, Color::Yellow);
        assert_eq!(config.directory.color, Color::Blue);
        Ok(())
    }

    #[test]
    fn test_config_connector_overrides() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[connectors]
git = "branch"
time = "time"
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.connectors.git, "branch");
        assert_eq!(config.connectors.time, "time");
        // Non-overridden connectors keep defaults
        assert_eq!(config.connectors.cmd_duration, "took");
        Ok(())
    }

    #[test]
    fn test_config_time_format_show_seconds() {
        let mut config = TimeConfig::default();
        assert!(config.show_seconds());

        config.format = TimeFormat::WithoutSeconds;
        assert!(!config.show_seconds());

        config.format = TimeFormat::WithSeconds;
        assert!(config.show_seconds());
    }

    #[test]
    fn test_config_time_format_deserialization() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[time]
format = "HH:MM"
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.time.format, TimeFormat::WithoutSeconds);
        Ok(())
    }

    #[test]
    fn test_config_time_format_invalid_returns_defaults() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[time]
format = "INVALID"
"#,
        )?;
        // Invalid format causes parse error → defaults
        let config = load_config(&path);
        assert_eq!(config.time.format, TimeFormat::WithSeconds);
        Ok(())
    }

    #[test]
    fn test_config_regex_pattern_invalid_returns_defaults() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[module]]
name = "bad"

[[module.source]]
command = ["echo", "x"]
regex = "(unclosed"
"#,
        )?;
        // Invalid regex causes parse error → defaults
        let config = load_config(&path);
        assert!(config.module.is_empty());
        Ok(())
    }

    #[test]
    fn test_config_git_icon_override() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[git]
icon = ""
indicator_color = "yellow"
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.git.icon, "");
        assert_eq!(config.git.indicator_color, Color::Yellow);
        Ok(())
    }

    // -- [[module]] config tests -----------------------------------------------

    #[test]
    fn test_config_module_empty_by_default() {
        let config = Config::default();
        assert!(config.module.is_empty());
    }

    #[test]
    fn test_config_module_parse_env_source() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[module]]
name = "aws"
when.env = ["AWS_PROFILE"]
format = "{value}"

[[module.source]]
env = "AWS_PROFILE"
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.module.len(), 1);
        assert_eq!(config.module[0].name, "aws");
        assert_eq!(config.module[0].when.env, ["AWS_PROFILE"]);
        assert_eq!(config.module[0].format, "{value}");
        assert_eq!(config.module[0].source.len(), 1);
        assert_eq!(
            config.module[0].source[0].env.as_deref(),
            Some("AWS_PROFILE")
        );
        assert!(!config.module[0].source[0].is_command());
        Ok(())
    }

    #[test]
    fn test_config_module_parse_command_source_with_regex() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[module]]
name = "zig"
when.files = ["build.zig"]
format = "v{value}"
icon = "Z"
color = "yellow"
connector = "via"

[[module.source]]
command = ["zig", "version"]
regex = '(\d[\d.]*)'
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.module.len(), 1);
        let m = &config.module[0];
        assert_eq!(m.name, "zig");
        assert_eq!(m.when.files, ["build.zig"]);
        assert_eq!(m.format, "v{value}");
        assert_eq!(m.icon.as_deref(), Some("Z"));
        assert_eq!(m.color, Some(Color::Yellow));
        assert_eq!(m.connector.as_deref(), Some("via"));
        assert_eq!(m.source.len(), 1);
        assert!(m.source[0].is_command());
        assert!(m.source[0].regex.is_some());
        Ok(())
    }

    #[test]
    fn test_config_module_default_format() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[module]]
name = "test"

[[module.source]]
env = "FOO"
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.module[0].format, "{value}");
        Ok(())
    }

    #[test]
    fn test_config_module_multiple_sources() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[[module]]
name = "node"
when.files = ["package.json"]
format = "v{value}"
connector = "via"

[[module.source]]
file = ".node-version"

[[module.source]]
command = ["node", "--version"]
regex = 'v?(\d[\d.]*)'
"#,
        )?;
        let config = load_config(&path);
        assert_eq!(config.module[0].source.len(), 2);
        assert!(config.module[0].source[0].file.is_some());
        assert!(config.module[0].source[1].command.is_some());
        Ok(())
    }
}
