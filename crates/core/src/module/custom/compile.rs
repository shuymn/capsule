use regex_lite::Regex;

use super::{super::ModuleSpeed, ResolvedModule, ResolvedSource};
use crate::{
    config::{ModuleDef, SourceDef},
    render::style::{Color, Style},
};

/// Compiles `[[module]]` entries into resolved modules with validated regexes.
#[must_use]
pub fn resolve_modules(modules: &[ModuleDef]) -> Vec<ResolvedModule> {
    modules.iter().cloned().map(compile_module_def).collect()
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
