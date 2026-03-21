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

/// Time module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct TimeConfig {
    /// Whether the time segment is displayed.
    pub enabled: bool,
    /// Time format string. Supported: `"HH:MM:SS"` (default), `"HH:MM"`.
    pub format: String,
    /// Foreground color for the time segment.
    pub color: Color,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            format: "HH:MM:SS".to_owned(),
            color: Color::Yellow,
        }
    }
}

impl TimeConfig {
    /// Whether seconds should be shown in the time output.
    #[must_use]
    pub fn show_seconds(&self) -> bool {
        self.format != "HH:MM"
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

/// Connector words between prompt segments.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorConfig {
    /// Connector before git segment.
    pub git: String,
    /// Connector before toolchain segment.
    pub toolchain: String,
    /// Connector before time segment.
    pub time: String,
    /// Connector before `cmd_duration` segment.
    pub cmd_duration: String,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            git: "on".to_owned(),
            toolchain: "via".to_owned(),
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
        assert_eq!(config.time.format, "HH:MM:SS");
        assert_eq!(config.time.color, Color::Yellow);
        assert_eq!(config.cmd_duration.threshold_ms, 2000);
        assert_eq!(config.cmd_duration.color, Color::Yellow);
        assert_eq!(config.connectors.git, "on");
        assert_eq!(config.connectors.toolchain, "via");
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
        assert_eq!(config.connectors.toolchain, "via");
        assert_eq!(config.connectors.cmd_duration, "took");
        Ok(())
    }

    #[test]
    fn test_config_time_format_show_seconds() {
        let mut config = TimeConfig::default();
        assert!(config.show_seconds());

        config.format = "HH:MM".to_owned();
        assert!(!config.show_seconds());

        config.format = "HH:MM:SS".to_owned();
        assert!(config.show_seconds());
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
}
