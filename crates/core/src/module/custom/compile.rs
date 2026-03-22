use regex_lite::Regex;

use super::{
    super::ModuleSpeed, ResolvedModule, ResolvedSource, ResolvedSourceGroup, detect::parse_format,
};
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
    let source_groups = group_sources(def.source);
    let format_segments = parse_format(&def.format);
    let speed = {
        let all_fast = source_groups
            .iter()
            .flat_map(|g| &g.sources)
            .all(ResolvedSource::is_fast);
        if all_fast {
            ModuleSpeed::Fast
        } else {
            ModuleSpeed::Slow
        }
    };
    let style = def
        .style
        .resolve(Style::new().fg(Color::BrightBlack).bold());

    ResolvedModule {
        name: def.name,
        when: def.when,
        source_groups,
        format_segments,
        icon: def.icon,
        style,
        connector: def.connector,
        speed,
        arbitration: def.arbitration,
    }
}

/// Groups a flat list of [`SourceDef`]s by variable name, preserving first-appearance order.
fn group_sources(sources: Vec<SourceDef>) -> Vec<ResolvedSourceGroup> {
    let mut groups: Vec<ResolvedSourceGroup> = Vec::new();
    for def in sources {
        let name = def.name.clone();
        let Some(resolved) = compile_source(def) else {
            continue;
        };
        if let Some(group) = groups.iter_mut().find(|g| g.name == name) {
            group.sources.push(resolved);
        } else {
            groups.push(ResolvedSourceGroup {
                name,
                sources: vec![resolved],
            });
        }
    }
    groups
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
