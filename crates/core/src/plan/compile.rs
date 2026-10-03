use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path, PathBuf},
};

use regex_lite::RegexBuilder;

use super::{
    Arbitration, Candidate, CharacterConfig, CmdDurationConfig, ConfigError, ConfigPlan,
    ConnectorConfig, DirectoryConfig, GitConfig, MAX_CANDIDATES_PER_VALUE, MAX_CONFIG_BYTES,
    MAX_MODULES, MAX_TOTAL_CANDIDATES, MAX_TOTAL_VALUES, MAX_VALUES_PER_MODULE, ModuleAppearance,
    ModulePlan, ModuleSlot, ModuleWhen, Source, StyleConfig, TimeConfig, ValuePlan, ViewConfig,
    template,
};
use crate::render::style::{Color, ColorMap, Style};

const MAX_WHEN_ITEMS: usize = 64;
const MAX_COMMAND_ARGS: usize = 64;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    schema_version: u32,
    #[serde(default)]
    character: CharacterConfig,
    #[serde(default)]
    directory: DirectoryConfig,
    #[serde(default)]
    git: GitConfig,
    #[serde(default)]
    time: TimeConfig,
    #[serde(default)]
    cmd_duration: CmdDurationConfig,
    #[serde(default)]
    connectors: ConnectorConfig,
    #[serde(default)]
    color_map: ColorMap,
    #[serde(default)]
    module: Vec<RawModule>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModule {
    name: String,
    #[serde(default)]
    when: ModuleWhen,
    values: BTreeMap<String, Vec<RawCandidate>>,
    #[serde(default = "default_format")]
    format: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    style: StyleConfig,
    #[serde(default)]
    connector: Option<String>,
    #[serde(default)]
    arbitration: Option<Arbitration>,
    #[serde(default)]
    slot: ModuleSlot,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCandidate {
    env: Option<String>,
    file: Option<PathBuf>,
    command: Option<Vec<String>>,
    regex: Option<String>,
}

pub(super) fn parse(source: &str) -> Result<ConfigPlan, ConfigError> {
    if source.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::invalid("configuration", "exceeds byte limit"));
    }
    let raw: RawConfig = toml::from_str(source)?;
    if raw.schema_version != 2 {
        return Err(ConfigError::invalid("schema_version", "expected 2"));
    }
    if raw.module.len() > MAX_MODULES {
        return Err(ConfigError::invalid("module", "too many modules"));
    }
    let mut names = HashSet::new();
    let mut total_values = 0;
    let mut total_candidates = 0;
    for module in &raw.module {
        if !names.insert(&module.name) {
            return Err(ConfigError::invalid(&module.name, "duplicate module name"));
        }
        total_values += module.values.len();
        total_candidates += module.values.values().map(Vec::len).sum::<usize>();
    }
    if total_values > MAX_TOTAL_VALUES || total_candidates > MAX_TOTAL_CANDIDATES {
        return Err(ConfigError::invalid(
            "module.values",
            "too many values or candidates across the plan",
        ));
    }
    let modules = raw
        .module
        .into_iter()
        .map(compile_module)
        .collect::<Result<_, _>>()?;
    Ok(ConfigPlan {
        view: ViewConfig {
            character: raw.character.merge_style_defaults(),
            directory: raw.directory.merge_style_defaults(),
            git: raw.git.merge_style_defaults(),
            time: raw.time.merge_style_defaults(),
            cmd_duration: raw.cmd_duration.merge_style_defaults(),
            connectors: raw.connectors,
            color_map: raw.color_map,
        },
        modules,
    })
}

fn compile_module(raw: RawModule) -> Result<ModulePlan, ConfigError> {
    validate_name(&raw.name, "module name")?;
    if raw.values.len() > MAX_VALUES_PER_MODULE {
        return Err(ConfigError::invalid(&raw.name, "too many named values"));
    }
    validate_when(&raw.when, &raw.name)?;
    if let Some(arbitration) = &raw.arbitration {
        validate_name(&arbitration.group, "arbitration group")?;
    }
    let values: Vec<ValuePlan> = raw
        .values
        .into_iter()
        .map(|(name, candidates)| {
            validate_name(&name, &raw.name)?;
            if candidates.is_empty() || candidates.len() > MAX_CANDIDATES_PER_VALUE {
                return Err(ConfigError::invalid(
                    &raw.name,
                    "each value needs 1 to 8 candidates",
                ));
            }
            let context = format!("module.{}.values.{name}", raw.name);
            Ok(ValuePlan {
                name,
                candidates: candidates
                    .into_iter()
                    .map(|candidate| compile_candidate(candidate, &context))
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<_, ConfigError>>()?;
    let format = template::compile(&raw.format, &values, &raw.name)?;
    Ok(ModulePlan {
        name: raw.name,
        when: raw.when,
        values,
        format,
        appearance: ModuleAppearance {
            icon: raw.icon,
            style: raw
                .style
                .merge_with(StyleConfig::fg_bold(Color::BrightBlack))
                .resolve(Style::new()),
            connector: raw.connector,
        },
        arbitration: raw.arbitration,
        slot: raw.slot,
    })
}

fn compile_candidate(raw: RawCandidate, context: &str) -> Result<Candidate, ConfigError> {
    let source = match (raw.env, raw.file, raw.command) {
        (Some(name), None, None) => {
            validate_env(&name, context)?;
            Source::Env(name)
        }
        (None, Some(path), None) => {
            validate_path(&path, context)?;
            Source::File(path)
        }
        (None, None, Some(args)) => {
            if args.is_empty()
                || args.len() > MAX_COMMAND_ARGS
                || args.first().is_none_or(String::is_empty)
                || args.iter().any(|arg| arg.contains('\0'))
            {
                return Err(ConfigError::invalid(context, "invalid command argv"));
            }
            Source::Command(args)
        }
        _ => {
            return Err(ConfigError::invalid(
                context,
                "each candidate must set exactly one of env, file, or command",
            ));
        }
    };
    let regex = raw
        .regex
        .map(|pattern| {
            let regex = RegexBuilder::new(&pattern)
                .size_limit(MAX_CONFIG_BYTES)
                .nest_limit(32)
                .build()
                .map_err(|source| ConfigError::Regex {
                    context: context.to_owned(),
                    source,
                })?;
            if regex.captures_len() < 2 {
                return Err(ConfigError::invalid(
                    context,
                    "regex must contain a capture group",
                ));
            }
            Ok(regex)
        })
        .transpose()?;
    Ok(Candidate { source, regex })
}

fn validate_when(when: &ModuleWhen, context: &str) -> Result<(), ConfigError> {
    if when.files.len() > MAX_WHEN_ITEMS || when.env.len() > MAX_WHEN_ITEMS {
        return Err(ConfigError::invalid(context, "too many conditions"));
    }
    for path in &when.files {
        validate_path(path, context)?;
    }
    for name in &when.env {
        validate_env(name, context)?;
    }
    Ok(())
}

fn validate_path(path: &Path, context: &str) -> Result<(), ConfigError> {
    if path.as_os_str().is_empty()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path
            .as_os_str()
            .as_encoded_bytes()
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(ConfigError::invalid(
            context,
            "file must be a relative path without . or .. components",
        ));
    }
    Ok(())
}

fn validate_env(name: &str, context: &str) -> Result<(), ConfigError> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return Err(ConfigError::invalid(context, "invalid environment name"));
    }
    Ok(())
}

fn validate_name(name: &str, context: &str) -> Result<(), ConfigError> {
    if name.is_empty()
        || name.trim() != name
        || name.chars().any(char::is_control)
        || name.contains(['{', '}', '[', ']'])
    {
        return Err(ConfigError::invalid(context, "invalid name"));
    }
    Ok(())
}

fn default_format() -> String {
    "{value}".to_owned()
}
