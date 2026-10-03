//! Display configuration shared by compiled plans and the view.

use crate::render::style::{Color, Style};

/// A partially specified prompt style override.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StyleConfig {
    /// Optional symbolic foreground color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<Color>,
    /// Optional bold override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    /// Optional dimmed override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimmed: Option<bool>,
}

impl StyleConfig {
    /// Returns a `StyleConfig` with only foreground color set.
    #[must_use]
    pub const fn fg(color: Color) -> Self {
        Self {
            fg: Some(color),
            bold: None,
            dimmed: None,
        }
    }

    /// Returns a `StyleConfig` with foreground color and bold enabled.
    #[must_use]
    pub const fn fg_bold(color: Color) -> Self {
        Self {
            fg: Some(color),
            bold: Some(true),
            dimmed: None,
        }
    }

    /// Fills in `None` fields from `defaults`, leaving explicitly set fields unchanged.
    pub(super) fn merge_with(self, defaults: Self) -> Self {
        Self {
            fg: self.fg.or(defaults.fg),
            bold: self.bold.or(defaults.bold),
            dimmed: self.dimmed.or(defaults.dimmed),
        }
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "Option equality is not const-stable on the current toolchain"
    )]
    #[must_use]
    pub fn resolve(&self, base: Style) -> Style {
        let mut style = base;
        if let Some(color) = self.fg {
            style = style.fg(color);
        }
        if matches!(self.bold, Some(true)) {
            style = style.bold();
        }
        if matches!(self.dimmed, Some(true)) {
            style = style.dimmed();
        }
        style
    }
}

/// Character prompt settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CharacterConfig {
    /// Whether the character module is disabled.
    pub disabled: bool,
    /// The prompt character glyph.
    pub glyph: String,
    /// Style for the success glyph (last command succeeded).
    pub success_style: StyleConfig,
    /// Style for the error glyph (last command failed).
    pub error_style: StyleConfig,
    /// Vi command mode override.
    #[serde(default)]
    pub vicmd: CharacterModeConfig,
}

/// Per-keymap character override (glyph and optional style).
///
/// When `style` is `Some`, it is used regardless of exit code.
/// When `style` is `None`, the parent [`CharacterConfig`]'s
/// `success_style` / `error_style` is used based on exit code.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CharacterModeConfig {
    /// The glyph displayed in this mode (default: `❮`).
    pub glyph: String,
    /// Fixed style for this mode (exit code independent).
    ///
    /// `None` falls back to the parent's `success_style`/`error_style`.
    pub style: Option<StyleConfig>,
}

impl Default for CharacterModeConfig {
    fn default() -> Self {
        Self {
            glyph: "\u{276e}".to_owned(),
            style: None,
        }
    }
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            glyph: "\u{276f}".to_owned(),
            success_style: StyleConfig::fg_bold(Color::Green),
            error_style: StyleConfig::fg_bold(Color::Red),
            vicmd: CharacterModeConfig::default(),
        }
    }
}

impl CharacterConfig {
    #[must_use]
    pub fn success_prompt_style(&self) -> Style {
        self.success_style.resolve(Style::new())
    }

    #[must_use]
    pub fn error_prompt_style(&self) -> Style {
        self.error_style.resolve(Style::new())
    }

    pub(super) fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.success_style = self.success_style.merge_with(defaults.success_style);
        self.error_style = self.error_style.merge_with(defaults.error_style);
        self
    }
}

/// Directory module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DirectoryConfig {
    /// Whether the directory module is disabled.
    pub disabled: bool,
    /// Style for the directory path.
    pub style: StyleConfig,
    /// Style for the readonly lock indicator.
    pub read_only_style: StyleConfig,
}

impl Default for DirectoryConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            style: StyleConfig::fg_bold(Color::Cyan),
            read_only_style: StyleConfig::fg(Color::Red),
        }
    }
}

impl DirectoryConfig {
    #[must_use]
    pub fn prompt_style(&self) -> Style {
        self.style.resolve(Style::new())
    }

    #[must_use]
    pub fn read_only_prompt_style(&self) -> Style {
        self.read_only_style.resolve(Style::new())
    }

    pub(super) fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self.read_only_style = self.read_only_style.merge_with(defaults.read_only_style);
        self
    }
}

/// Git module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    /// Whether the git module is disabled.
    pub disabled: bool,
    /// Nerd Font icon glyph for git branch.
    pub icon: String,
    /// Connector word before the git segment (e.g., `"on"`).
    pub connector: String,
    /// Style for the branch text and icon.
    pub style: StyleConfig,
    /// Style for status indicators (e.g., `[!+]`).
    pub indicator_style: StyleConfig,
    /// Style for operation state labels (e.g., `(REBASING 2/5)`).
    pub state_style: StyleConfig,
    /// Style for `(hash)` in detached `HEAD (hash)` output.
    pub detached_hash_style: StyleConfig,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            icon: "\u{f418}".to_owned(),
            connector: "on".to_owned(),
            style: StyleConfig::fg_bold(Color::Magenta),
            indicator_style: StyleConfig::fg_bold(Color::Red),
            detached_hash_style: StyleConfig::fg_bold(Color::Green),
            state_style: StyleConfig::fg_bold(Color::Yellow),
        }
    }
}

impl GitConfig {
    #[must_use]
    pub fn prompt_style(&self) -> Style {
        self.style.resolve(Style::new())
    }

    #[must_use]
    pub fn indicator_prompt_style(&self) -> Style {
        self.indicator_style.resolve(Style::new())
    }

    #[must_use]
    pub fn detached_hash_prompt_style(&self) -> Style {
        self.detached_hash_style.resolve(Style::new())
    }

    #[must_use]
    pub fn state_prompt_style(&self) -> Style {
        self.state_style.resolve(Style::new())
    }

    pub(super) fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self.indicator_style = self.indicator_style.merge_with(defaults.indicator_style);
        self.state_style = self.state_style.merge_with(defaults.state_style);
        self.detached_hash_style = self
            .detached_hash_style
            .merge_with(defaults.detached_hash_style);
        self
    }
}

/// Supported time display formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
pub enum TimeFormat {
    /// `HH:MM:SS` — hours, minutes, seconds.
    #[strum(serialize = "HH:MM:SS")]
    WithSeconds,
    /// `HH:MM` — hours and minutes only.
    #[strum(serialize = "HH:MM")]
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
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(|error| {
            serde::de::Error::custom(format!(
                "unsupported time format `{value}`: {error}; expected \"{}\" or \"{}\"",
                Self::WithSeconds,
                Self::WithoutSeconds
            ))
        })
    }
}

/// Time module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeConfig {
    /// Whether the time module is disabled.
    pub disabled: bool,
    /// Time format.
    pub format: TimeFormat,
    /// Connector word before the time segment (e.g., `"at"`).
    pub connector: String,
    /// Style for the time segment.
    pub style: StyleConfig,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            disabled: true,
            format: TimeFormat::WithSeconds,
            connector: "at".to_owned(),
            style: StyleConfig::fg_bold(Color::Yellow),
        }
    }
}

impl TimeConfig {
    /// Whether seconds should be shown in the time output.
    #[must_use]
    pub const fn show_seconds(&self) -> bool {
        self.format.show_seconds()
    }

    #[must_use]
    pub fn prompt_style(&self) -> Style {
        self.style.resolve(Style::new())
    }

    pub(super) fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self
    }
}

/// Command duration module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CmdDurationConfig {
    /// Whether the command duration module is disabled.
    pub disabled: bool,
    /// Minimum duration in milliseconds before showing the segment.
    pub threshold_ms: u64,
    /// Connector word before the duration segment (e.g., `"took"`).
    pub connector: String,
    /// Style for the duration segment.
    pub style: StyleConfig,
}

impl Default for CmdDurationConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            threshold_ms: 2000,
            connector: "took".to_owned(),
            style: StyleConfig::fg_bold(Color::Yellow),
        }
    }
}

impl CmdDurationConfig {
    #[must_use]
    pub fn prompt_style(&self) -> Style {
        self.style.resolve(Style::new())
    }

    pub(super) fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self
    }
}

/// Shared style for connector words between prompt segments.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectorConfig {
    /// Structured style override for all connector words.
    pub style: StyleConfig,
}

impl ConnectorConfig {
    #[must_use]
    pub fn prompt_style(&self) -> Style {
        self.style.resolve(Style::new())
    }
}
