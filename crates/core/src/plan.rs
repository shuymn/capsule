//! Validated schema-v2 plans, observations, and ordered value acquisition.

mod acquire;
mod compile;
mod style;
mod template;

use std::path::PathBuf;

pub use acquire::{acquire_condition, acquire_value};
use regex_lite::Regex;
pub use style::{
    CharacterConfig, CharacterModeConfig, CmdDurationConfig, ConnectorConfig, DirectoryConfig,
    GitConfig, StyleConfig, TimeConfig, TimeFormat,
};

use crate::{
    acquire::AcquireError,
    render::style::{ColorMap, Style},
};

/// Maximum TOML input size accepted before deserialization.
pub const MAX_CONFIG_BYTES: usize = crate::acquire::MAX_OUTPUT_BYTES;
/// Maximum number of custom modules in a plan.
pub const MAX_MODULES: usize = 64;
/// Maximum named values in one module.
pub const MAX_VALUES_PER_MODULE: usize = 16;
/// Maximum named values across the entire plan.
pub const MAX_TOTAL_VALUES: usize = 256;
/// Maximum fallback candidates for one named value.
pub const MAX_CANDIDATES_PER_VALUE: usize = 8;
/// Maximum fallback candidates across the entire plan.
pub const MAX_TOTAL_CANDIDATES: usize = 1024;
/// Maximum format source length, in bytes.
pub const MAX_FORMAT_BYTES: usize = 4096;
/// Maximum nested optional format sections.
pub const MAX_FORMAT_DEPTH: usize = 8;

/// Immutable-by-convention compiled configuration shared by one generation.
#[derive(Debug, Clone, Default)]
pub struct ConfigPlan {
    /// Built-in display configuration.
    pub view: ViewConfig,
    /// Custom modules in declaration order.
    pub modules: Vec<ModulePlan>,
}

impl ConfigPlan {
    /// Compile schema-v2 TOML without performing any I/O or running commands.
    ///
    /// # Errors
    ///
    /// Rejects unknown fields, invalid definitions, and bounded-plan violations.
    pub fn parse(source: &str) -> Result<Self, ConfigError> {
        compile::parse(source)
    }
}

/// Display settings whose defaults preserve the two-line prompt contract.
#[derive(Debug, Clone, Default)]
pub struct ViewConfig {
    /// Status and vi-mode character.
    pub character: CharacterConfig,
    /// Current directory and read-only indicator.
    pub directory: DirectoryConfig,
    /// Git branch, state, and indicators.
    pub git: GitConfig,
    /// Optional local clock.
    pub time: TimeConfig,
    /// Last command duration.
    pub cmd_duration: CmdDurationConfig,
    /// Shared connector style.
    pub connectors: ConnectorConfig,
    /// Symbolic foreground mapping.
    pub color_map: ColorMap,
}

/// A module whose sources and format references have already been validated.
#[derive(Debug, Clone)]
pub struct ModulePlan {
    /// Unique module name.
    pub name: String,
    /// Conditions checked before any value acquisition starts.
    pub when: ModuleWhen,
    /// Named values referenced by index from the format.
    pub values: Vec<ValuePlan>,
    /// Parsed format containing only validated references.
    pub format: Template,
    /// Structured display metadata.
    pub appearance: ModuleAppearance,
    /// Optional competition group, evaluated once by the view.
    pub arbitration: Option<Arbitration>,
    /// Prompt line placement.
    pub slot: ModuleSlot,
}

/// Appearance shared by all formatted values in one module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleAppearance {
    /// Optional icon preceding the content.
    pub icon: Option<String>,
    /// Resolved text and icon style.
    pub style: Style,
    /// Optional connector preceding the icon/content.
    pub connector: Option<String>,
}

/// One named value with a sequential fallback chain.
#[derive(Debug, Clone)]
pub struct ValuePlan {
    /// Name referenced by the TOML format.
    pub name: String,
    /// Candidates in configured order.
    pub candidates: Vec<Candidate>,
}

/// One validated source and optional first-capture extraction.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Acquisition operation.
    pub source: Source,
    /// Compiled pattern with at least one capture group.
    pub regex: Option<Regex>,
}

/// Generic source operations; commands always use argv without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Read the full exported environment snapshot.
    Env(String),
    /// Read bounded text from a path relative to the generation's cwd.
    File(PathBuf),
    /// Execute argv with the generation's cwd and full environment.
    Command(Vec<String>),
}

/// Conditions use OR within each list and AND between the lists.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModuleWhen {
    /// Marker files relative to cwd; an empty list imposes no file condition.
    pub files: Vec<PathBuf>,
    /// Exported variable presence; an empty value still counts as present.
    pub env: Vec<String>,
}

/// One winner per group, preferring lower priority then declaration order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arbitration {
    /// Competition group name.
    pub group: String,
    /// Lower numbers win.
    pub priority: u32,
}

/// Custom module placement within the existing two prompt lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleSlot {
    /// After Git and before duration.
    #[default]
    Line1,
    /// Before time and the prompt character.
    Line2,
}

/// Format compiled into literal text, value indices, and optional sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template(pub Vec<FormatPart>);

/// Required unresolved values hide their containing section or module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatPart {
    /// Literal text, kept unescaped until final serialization.
    Literal(String),
    /// Index into the containing module's values/observations.
    Value(usize),
    /// Omit this section when any direct required value is unresolved.
    Optional(Vec<Self>),
}

/// Current acquisition state; empty ready values remain distinct from missing.
#[derive(Debug)]
pub enum Observation<T> {
    /// Work has not completed.
    Pending,
    /// A successful value, including empty strings and false conditions.
    Ready(T),
    /// All candidates were absent or their regex did not match.
    Missing,
    /// Acquisition failed and no later candidate succeeded.
    Failed(AcquireError),
}

/// Observations belong to one module in one execution generation.
#[derive(Debug)]
pub struct ModuleObservation {
    /// Only `Ready(true)` makes the module eligible for display/acquisition.
    pub condition: Observation<bool>,
    /// Values in the same order as the module's compiled value plans.
    pub values: Vec<Observation<String>>,
}

impl ModuleObservation {
    /// Allocate the initial state for a new acquisition generation.
    #[must_use]
    pub fn pending(module: &ModulePlan) -> Self {
        Self {
            condition: Observation::Pending,
            values: module.values.iter().map(|_| Observation::Pending).collect(),
        }
    }
}

/// Configuration failures preserve the last successfully compiled plan.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Invalid TOML or a rejected field/type.
    #[error("invalid configuration: {0}")]
    Toml(#[from] toml::de::Error),
    /// Semantically invalid or oversized configuration.
    #[error("invalid {context}: {reason}")]
    Invalid {
        /// Configuration location or bound.
        context: String,
        /// Failure without environment or command-output data.
        reason: &'static str,
    },
    /// Regex syntax/size validation failed.
    #[error("invalid regex in {context}: {source}")]
    Regex {
        /// Owning module/value.
        context: String,
        /// Regex compiler error.
        #[source]
        source: regex_lite::Error,
    },
}

impl ConfigError {
    fn invalid(context: &str, reason: &'static str) -> Self {
        Self::Invalid {
            context: context.to_owned(),
            reason,
        }
    }
}

#[cfg(test)]
mod tests;
