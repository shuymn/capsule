//! Configuration file loading and defaults.
//!
//! Reads `$XDG_CONFIG_HOME/capsule/config.toml` (fallback `~/.capsule/config.toml`).
//! Missing file → compiled-in defaults. Parse error → log + defaults.

use std::path::{Component, Path, PathBuf};

use crate::render::{
    segment::{Connector, Icon, Segment},
    style::{Color, ColorMap, Style},
};

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
    /// Timeout settings for module execution.
    pub timeout: TimeoutConfig,
    /// Mapping from symbolic colors to concrete ANSI foreground codes.
    pub color_map: ColorMap,
    /// Cache settings.
    pub cache: CacheConfig,
    /// User-defined prompt modules (`[[module]]` array).
    #[serde(default)]
    pub module: Vec<ModuleDef>,
}

/// A partially specified prompt style override.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
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
    fn merge_with(self, defaults: Self) -> Self {
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
#[serde(default)]
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
#[serde(default)]
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

    /// Resolve prompt style based on the last command's exit code.
    fn exit_style(&self, exit_code: i32) -> Style {
        if exit_code == 0 {
            self.success_prompt_style()
        } else {
            self.error_prompt_style()
        }
    }

    /// Build a [`Segment`] for the character glyph, styled by exit code.
    #[must_use]
    pub(crate) fn to_segment(&self, glyph: &str, exit_code: i32) -> Segment {
        Segment {
            content: glyph.to_owned(),
            connector: None,
            icon: None,
            content_style: Some(self.exit_style(exit_code)),
        }
    }

    /// Build a [`Segment`] for a keymap mode override.
    ///
    /// If the mode has its own `style`, it is used regardless of `exit_code`.
    /// Otherwise, falls back to this config's `success_style`/`error_style`.
    #[must_use]
    pub(crate) fn mode_segment(&self, mode: &CharacterModeConfig, exit_code: i32) -> Segment {
        let style = mode
            .style
            .as_ref()
            .map_or_else(|| self.exit_style(exit_code), |s| s.resolve(Style::new()));
        Segment {
            content: mode.glyph.clone(),
            connector: None,
            icon: None,
            content_style: Some(style),
        }
    }

    fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.success_style = self.success_style.merge_with(defaults.success_style);
        self.error_style = self.error_style.merge_with(defaults.error_style);
        self
    }
}

/// Directory module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
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

    /// Build a [`Segment`] for the directory path.
    ///
    /// When `read_only` is true, the content is pre-styled (mixed styles for
    /// path and lock icon), so `content_style` is `None`.
    #[must_use]
    pub(crate) fn to_segment(&self, dir: &str, read_only: bool, color_map: ColorMap) -> Segment {
        if read_only {
            let dir_style = self.prompt_style();
            let lock_style = self.read_only_prompt_style();
            let content = format!(
                "{} {}",
                dir_style.paint_with(dir, color_map),
                lock_style.paint_with("\u{f023}", color_map)
            );
            Segment {
                content,
                connector: None,
                icon: None,
                content_style: None,
            }
        } else {
            Segment {
                content: dir.to_owned(),
                connector: None,
                icon: None,
                content_style: Some(self.prompt_style()),
            }
        }
    }

    fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self.read_only_style = self.read_only_style.merge_with(defaults.read_only_style);
        self
    }
}

/// Git module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
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

    /// Build a [`Segment`] for git status output (already styled by the
    /// git module).
    #[must_use]
    pub(crate) fn to_segment(&self, git_output: &str, connector_style: Style) -> Segment {
        Segment {
            content: git_output.to_owned(),
            connector: Some(Connector {
                word: self.connector.clone(),
                style: connector_style,
            }),
            icon: Some(Icon {
                glyph: self.icon.clone(),
                style: self.prompt_style(),
            }),
            content_style: None,
        }
    }

    fn merge_style_defaults(mut self) -> Self {
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
#[serde(default)]
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

    /// Build a [`Segment`] for the time display.
    #[must_use]
    pub(crate) fn to_segment(&self, time_str: &str, connector_style: Style) -> Segment {
        Segment {
            content: time_str.to_owned(),
            connector: Some(Connector {
                word: self.connector.clone(),
                style: connector_style,
            }),
            icon: None,
            content_style: Some(self.prompt_style()),
        }
    }

    fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self
    }
}

/// Command duration module settings.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
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

    /// Build a [`Segment`] for the command duration display.
    #[must_use]
    pub(crate) fn to_segment(&self, duration_str: &str, connector_style: Style) -> Segment {
        Segment {
            content: duration_str.to_owned(),
            connector: Some(Connector {
                word: self.connector.clone(),
                style: connector_style,
            }),
            icon: None,
            content_style: Some(self.prompt_style()),
        }
    }

    fn merge_style_defaults(mut self) -> Self {
        let defaults = Self::default();
        self.style = self.style.merge_with(defaults.style);
        self
    }
}

/// A regex pattern validated at deserialization time.
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl serde::Serialize for RegexPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
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

/// Prompt line placement for a custom module.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleSlot {
    /// Line 1: after git, before command duration (default).
    #[default]
    Line1,
    /// Line 2: before time.
    Line2,
}

/// User-defined prompt module entry from `[[module]]` in config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Display style (fg, bold, dimmed).
    #[serde(default)]
    pub style: StyleConfig,
    /// Connector word before this segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    /// Optional arbitration metadata for collapsing competing modules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arbitration: Option<Arbitration>,
    /// Prompt line placement.
    #[serde(default)]
    pub slot: ModuleSlot,
}

fn default_module_format() -> String {
    "{value}".to_owned()
}

fn default_source_name() -> String {
    "value".to_owned()
}

/// Arbitration rule for collapsing competing modules into a single winner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Arbitration {
    /// Group identifier used to decide which modules compete.
    pub group: String,
    /// Lower numbers win within the same group.
    pub priority: u32,
}

/// Conditions that trigger a module.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ModuleWhen {
    /// Marker files whose presence in cwd triggers the module.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Environment variables whose presence triggers the module.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
}

/// A single value source within a module definition.
///
/// Exactly one of `env`, `file`, or `command` must be set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourceDef {
    /// Variable name this source contributes to.
    ///
    /// Sources with the same name form a fallback chain.
    /// Defaults to `"value"` when omitted.
    #[serde(default = "default_source_name")]
    pub name: String,
    /// Read value from an environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Read value from a file in cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Run a command and use its stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Regex applied to the source output; first capture group is the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<RegexPattern>,
}

impl SourceDef {
    /// Whether this source requires executing an external command.
    #[must_use]
    pub const fn is_command(&self) -> bool {
        self.command.is_some()
    }
}

impl<'de> serde::Deserialize<'de> for SourceDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawSourceDef {
            #[serde(default = "default_source_name")]
            name: String,
            #[serde(default)]
            env: Option<String>,
            #[serde(default)]
            file: Option<String>,
            #[serde(default)]
            command: Option<Vec<String>>,
            #[serde(default)]
            regex: Option<RegexPattern>,
        }

        let raw = RawSourceDef::deserialize(deserializer)?;
        let source_count = usize::from(raw.env.is_some())
            + usize::from(raw.file.is_some())
            + usize::from(raw.command.is_some());
        if source_count != 1 {
            return Err(serde::de::Error::custom(
                "module source must set exactly one of env, file, or command",
            ));
        }
        if let Some(path) = raw.file.as_deref()
            && !is_safe_relative_module_path(path)
        {
            return Err(serde::de::Error::custom(
                "module source file must be a relative path without . or .. components",
            ));
        }
        if raw.command.as_ref().is_some_and(Vec::is_empty) {
            return Err(serde::de::Error::custom(
                "module source command must contain at least one argument",
            ));
        }
        if raw
            .command
            .as_ref()
            .is_some_and(|args| args.iter().any(String::is_empty))
        {
            return Err(serde::de::Error::custom(
                "module source command arguments must not be empty",
            ));
        }

        Ok(Self {
            name: raw.name,
            env: raw.env,
            file: raw.file,
            command: raw.command,
            regex: raw.regex,
        })
    }
}

pub(crate) fn is_safe_relative_module_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Timeout settings for module execution.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct TimeoutConfig {
    /// Maximum time in milliseconds to wait for fast modules (env/file).
    pub fast_ms: u64,
    /// Maximum time in milliseconds to wait for slow modules (commands/git).
    pub slow_ms: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            fast_ms: 500,
            slow_ms: 5000,
        }
    }
}

/// Shared style for connector words between prompt segments.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
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

/// Caching strategy for slow module results (git, commands).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlowCacheMode {
    /// Do not cache slow module results; always compute fresh.
    Off,
    /// Cache slow module results but revalidate in background on every hit.
    #[default]
    Revalidate,
}

/// Cache settings.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Caching strategy for slow modules.
    pub slow: SlowCacheMode,
}

/// Errors while reading or parsing a configuration file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    /// Reading the config file failed.
    #[error("failed to read config `{path}`")]
    Read {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Parsing the config file failed.
    #[error("failed to parse config `{path}`")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying TOML parse error.
        #[source]
        source: toml::de::Error,
    },
}

/// Resolve the config file path.
///
/// Returns `None` if neither `$XDG_CONFIG_HOME` nor `$HOME` is set.
///
/// Resolution order:
/// 1. `$XDG_CONFIG_HOME/capsule/config.toml` (if `XDG_CONFIG_HOME` is set)
/// 2. `$HOME/.config/capsule/config.toml`
/// 3. `$HOME/.capsule/config.toml`
///
/// When neither candidate file exists, returns `$HOME/.config/capsule/config.toml`
/// so that the daemon's hot-reload watcher can detect a newly created file.
#[must_use]
pub fn resolve_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("capsule/config.toml"));
    }
    let home = PathBuf::from(std::env::var("HOME").ok()?);

    let xdg_default = home.join(".config/capsule/config.toml");
    if xdg_default.exists() {
        return Some(xdg_default);
    }

    let dotdir = home.join(".capsule/config.toml");
    if dotdir.exists() {
        return Some(dotdir);
    }

    Some(xdg_default)
}

impl Config {
    /// Merges partially-specified nested [`StyleConfig`] fields with their parent defaults.
    ///
    /// When a user writes `[character.success_style]\nfg = "magenta"`, serde fills in
    /// missing `StyleConfig` fields from `StyleConfig::default()` (all `None`) rather than
    /// from `CharacterConfig::default().success_style`. This method restores those defaults
    /// so that, for example, `bold = true` is preserved when only `fg` is overridden.
    fn merge_style_defaults(mut self) -> Self {
        self.character = self.character.merge_style_defaults();
        self.directory = self.directory.merge_style_defaults();
        self.git = self.git.merge_style_defaults();
        self.time = self.time.merge_style_defaults();
        self.cmd_duration = self.cmd_duration.merge_style_defaults();
        self
    }
}

/// Load configuration from the given path.
///
/// - If the file does not exist, returns compiled-in defaults.
/// - If the file has syntax errors, logs the error and returns defaults.
pub fn load_config(path: &Path) -> Config {
    match read_config(path) {
        Ok(Some(config)) => config,
        Ok(None) => Config::default(),
        Err(ConfigLoadError::Parse { path, source }) => {
            eprintln!(
                "capsule: failed to parse config `{}`: {source}; using defaults",
                path.display()
            );
            tracing::error!(path = %path.display(), error = %source, "config parse error, using defaults");
            Config::default()
        }
        Err(ConfigLoadError::Read { path, source }) => {
            tracing::error!(path = %path.display(), error = %source, "failed to read config, using defaults");
            Config::default()
        }
    }
}

/// Read configuration from the given path without falling back to defaults.
///
/// Returns `Ok(None)` when the file does not exist.
///
/// # Errors
///
/// Returns [`ConfigLoadError`] when the file cannot be read or parsed.
pub fn read_config(path: &Path) -> Result<Option<Config>, ConfigLoadError> {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str::<Config>(&content)
            .map(|config| Some(config.merge_style_defaults()))
            .map_err(|source| ConfigLoadError::Parse {
                path: path.to_path_buf(),
                source,
            }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigLoadError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Load configuration from the default resolved path, or return defaults.
#[must_use]
pub fn load_default_config() -> Config {
    resolve_config_path().map_or_else(Config::default, |path| load_config(&path))
}

#[cfg(test)]
mod tests;
