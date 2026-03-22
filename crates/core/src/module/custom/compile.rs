use regex_lite::Regex;

use super::{super::ModuleSpeed, ResolvedModule, ResolvedSource, builtins::builtin_module_defs};
use crate::{
    config::{ModuleDef, SourceDef},
    render::style::{Color, Style},
};

/// Merges built-in modules with user-defined `[[module]]` entries and compiles
/// regexes.
///
/// Order: built-in toolchains (as modules) first, then user additions.
/// Same-name entries replace in-place (preserving position).
#[must_use]
pub fn resolve_modules(user_modules: &[ModuleDef]) -> Vec<ResolvedModule> {
    let mut defs = builtin_module_defs();

    for user_module in user_modules {
        if let Some(existing) = defs.iter_mut().find(|def| def.name == user_module.name) {
            *existing = user_module.clone();
        } else {
            defs.push(user_module.clone());
        }
    }

    defs.into_iter().map(compile_module_def).collect()
}

fn compile_module_def(def: ModuleDef) -> ResolvedModule {
    let sources: Vec<ResolvedSource> = def.source.into_iter().filter_map(compile_source).collect();
    let speed = if sources.iter().all(ResolvedSource::is_fast) {
        ModuleSpeed::Fast
    } else {
        ModuleSpeed::Slow
    };
    let style = def
        .style
        .resolve(Style::new().fg(Color::BrightBlack).bold());

    ResolvedModule {
        name: def.name,
        when: def.when,
        sources,
        format: def.format,
        icon: def.icon,
        style,
        connector: def.connector,
        speed,
        arbitration: def.arbitration,
    }
}

fn compile_source(def: SourceDef) -> Option<ResolvedSource> {
    let regex = def
        .regex
        .as_ref()
        .and_then(|pattern| Regex::new(pattern.as_str()).ok());

    if let Some(env_name) = def.env {
        Some(ResolvedSource::Env {
            name: env_name,
            regex,
        })
    } else if let Some(file_path) = def.file {
        Some(ResolvedSource::File {
            path: file_path,
            regex,
        })
    } else if let Some(args) = def.command {
        if args.is_empty() {
            None
        } else {
            Some(ResolvedSource::Command { args, regex })
        }
    } else {
        None
    }
}
