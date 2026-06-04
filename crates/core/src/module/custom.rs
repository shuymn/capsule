//! Custom prompt modules — user-defined segments via `[[module]]` config DSL.
//!
//! Each [`ResolvedModule`] defines trigger conditions, value sources, and
//! display metadata. Sources within the same named fallback group are tried in
//! declaration order.

mod builtins;
mod compile;
mod detect;
mod facts;

use std::{path::PathBuf, sync::Arc};

pub use builtins::preset_module_defs;
pub use compile::resolve_modules;
pub(crate) use detect::arbitrate_detected_modules;
pub use detect::detect_modules;
pub use facts::required_env_var_names;
use regex_lite::Regex;

use super::ModuleSpeed;
use crate::{
    config::{Arbitration, ModuleWhen},
    render::{
        segment::{Connector, Icon, Segment},
        style::Style,
    },
};

/// A compiled module definition ready for detection.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// Module identifier.
    pub name: String,
    /// Trigger conditions.
    pub when: ModuleWhen,
    /// Compiled value source groups, keyed by variable name.
    pub source_groups: Vec<ResolvedSourceGroup>,
    /// Pre-parsed format segments.
    pub format_segments: Vec<detect::FormatSegment>,
    /// Nerd Font icon glyph.
    pub icon: Option<String>,
    /// Display style.
    pub style: Style,
    /// Connector word before this segment.
    pub connector: Option<String>,
    /// Computed speed: fast if all sources are env/file, slow if any command.
    pub speed: ModuleSpeed,
    /// Optional arbitration rule for collapsing competing detected modules.
    pub arbitration: Option<Arbitration>,
}

impl ResolvedModule {
    /// Iterates over all sources across all groups.
    pub(crate) fn all_sources(&self) -> impl Iterator<Item = &ResolvedSource> {
        self.source_groups.iter().flat_map(|g| &g.sources)
    }
}

/// A named group of fallback sources that resolve to a single format variable.
#[derive(Debug, Clone)]
pub struct ResolvedSourceGroup {
    /// Variable name used in the format string (e.g. `"version"`, `"region"`).
    pub name: String,
    /// Ordered fallback sources for this variable.
    pub sources: Vec<ResolvedSource>,
}

/// A compiled value source.
#[derive(Debug, Clone)]
pub enum ResolvedSource {
    /// Read from an environment variable.
    Env { name: String, regex: Option<Regex> },
    /// Read from a file in cwd.
    File { path: String, regex: Option<Regex> },
    /// Run a command.
    Command {
        args: Vec<String>,
        regex: Option<Regex>,
    },
}

impl ResolvedSource {
    const fn is_fast(&self) -> bool {
        !matches!(self, Self::Command { .. })
    }
}

/// Detected custom module with resolved value and display metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomModuleInfo {
    /// Module identifier.
    pub name: String,
    /// Formatted value string.
    pub value: String,
    /// Nerd Font icon glyph.
    pub icon: Option<String>,
    /// Display style.
    pub style: Style,
    /// Connector word.
    pub connector: Option<String>,
}

impl CustomModuleInfo {
    /// Build a [`Segment`] from this custom module info.
    #[must_use]
    pub(crate) fn to_segment(&self, connector_style: Style) -> Segment {
        let connector = self.connector.as_deref().map(|word| Connector {
            word: word.to_owned(),
            style: connector_style,
        });
        let icon = self.icon.as_deref().map(|glyph| Icon {
            glyph: glyph.to_owned(),
            style: self.style,
        });
        Segment {
            content: self.value.clone(),
            connector,
            icon,
            content_style: Some(self.style),
        }
    }
}

/// Candidate for arbitration, in definition order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedModuleCandidate {
    /// Optional arbitration rule for this detected module.
    pub(crate) arbitration: Option<Arbitration>,
    /// Detected module info.
    pub(crate) info: CustomModuleInfo,
}

impl DetectedModuleCandidate {
    pub(crate) fn new(module: &ResolvedModule, info: CustomModuleInfo) -> Self {
        Self {
            arbitration: module.arbitration.clone(),
            info,
        }
    }
}

/// Shared request-derived facts reused across module detection and prompt
/// rendering.
#[derive(Clone)]
pub(crate) struct RequestFacts {
    cwd: PathBuf,
    env_vars: Vec<(String, String)>,
    path_env: Option<String>,
    read_only: bool,
    command_resolver: Arc<facts::CommandResolver>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModuleDependencyInputs {
    pub(crate) env_vars: Vec<String>,
    pub(crate) trigger_files: Vec<String>,
    pub(crate) source_files: Vec<String>,
    pub(crate) uses_command_path: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
