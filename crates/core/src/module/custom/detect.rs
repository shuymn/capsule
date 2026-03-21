use std::{collections::HashMap, path::Path};

use regex_lite::Regex;

use super::{
    super::ModuleSpeed, CustomModuleInfo, DetectedModuleCandidate, RequestFacts, ResolvedModule,
};

/// Detects all matching custom modules for the given directory.
///
/// `env_vars` provides environment variable values forwarded from the shell.
/// `path_env` overrides PATH for command execution (launchd support).
///
/// Fast modules only try env/file sources. Slow modules try env/file first,
/// then command sources on failure.
#[must_use]
pub fn detect_modules(
    defs: &[ResolvedModule],
    cwd: &Path,
    env_vars: &[(String, String)],
    path_env: Option<&str>,
    only_speed: ModuleSpeed,
) -> Vec<CustomModuleInfo> {
    let facts = RequestFacts::collect(cwd.to_path_buf(), env_vars.to_vec())
        .with_command_path_env(path_env.map(ToOwned::to_owned));
    let detected = facts
        .matching_modules(defs, only_speed)
        .into_iter()
        .filter_map(|(_, module)| {
            facts
                .detect_module(module)
                .map(|info| DetectedModuleCandidate::new(module, info))
        })
        .collect();
    arbitrate_detected_modules(detected)
}

/// Collapse competing detected modules while preserving definition order.
#[must_use]
pub fn arbitrate_detected_modules(detected: Vec<DetectedModuleCandidate>) -> Vec<CustomModuleInfo> {
    let mut winners = HashMap::<String, (usize, u32)>::new();

    for (idx, candidate) in detected.iter().enumerate() {
        let Some(arbitration) = &candidate.arbitration else {
            continue;
        };
        winners
            .entry(arbitration.group.clone())
            .and_modify(|winner| {
                if arbitration.priority < winner.1 {
                    *winner = (idx, arbitration.priority);
                }
            })
            .or_insert((idx, arbitration.priority));
    }

    detected
        .into_iter()
        .enumerate()
        .filter_map(|(idx, candidate)| match &candidate.arbitration {
            None => Some(candidate.info),
            Some(arbitration) => winners
                .get(&arbitration.group)
                .is_some_and(|winner| winner.0 == idx)
                .then_some(candidate.info),
        })
        .collect()
}

pub(super) fn apply_regex(input: &str, regex: Option<&Regex>) -> Option<String> {
    if let Some(regex) = regex {
        let captures = regex.captures(input)?;
        Some(captures.get(1)?.as_str().to_owned())
    } else {
        Some(input.to_owned())
    }
}

/// Format placeholder for value substitution in module format strings.
const VALUE_PLACEHOLDER: &str = "{value}";

pub(super) fn make_info(def: &ResolvedModule, raw_value: &str) -> CustomModuleInfo {
    let value = def.format.replace(VALUE_PLACEHOLDER, raw_value);
    CustomModuleInfo {
        name: def.name.clone(),
        value,
        icon: def.icon.clone(),
        style: def.style,
        connector: def.connector.clone(),
    }
}
