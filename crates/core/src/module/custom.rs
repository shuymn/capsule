//! Custom prompt modules — user-defined segments via `[[module]]` config DSL.
//!
//! Each [`ResolvedModule`] defines trigger conditions, value sources, and
//! display metadata. Sources are tried in order: fast sources (env, file)
//! first, then slow sources (command) if fast sources all fail.

use std::{collections::HashSet, path::Path, process::Command};

use regex_lite::Regex;

use super::ModuleSpeed;
use crate::{
    config::{ModuleDef, ModuleWhen, SourceDef},
    render::style::{Color, Style},
};

// ---------------------------------------------------------------------------
// Resolved types (compiled from config, ready for detection)
// ---------------------------------------------------------------------------

/// A compiled module definition ready for detection.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// Module identifier.
    pub name: String,
    /// Trigger conditions.
    pub when: ModuleWhen,
    /// Compiled value sources.
    pub sources: Vec<ResolvedSource>,
    /// Format string with `{value}` placeholder.
    pub format: String,
    /// Nerd Font icon glyph.
    pub icon: Option<String>,
    /// Display style.
    pub style: Style,
    /// Connector word before this segment.
    pub connector: Option<String>,
    /// Computed speed: fast if all sources are env/file, slow if any command.
    pub speed: ModuleSpeed,
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

// ---------------------------------------------------------------------------
// Built-in toolchains as module definitions
// ---------------------------------------------------------------------------

/// Collect all environment variable names referenced by resolved modules.
///
/// Includes variables from `when.env` and env-type sources. Deduplicated.
#[must_use]
pub fn required_env_var_names(modules: &[ResolvedModule]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for m in modules {
        for e in &m.when.env {
            if seen.insert(e.as_str()) {
                names.push(e.clone());
            }
        }
        for s in &m.sources {
            if let ResolvedSource::Env { name, .. } = s
                && seen.insert(name.as_str())
            {
                names.push(name.clone());
            }
        }
    }
    names
}

/// Returns the 6 built-in toolchain definitions as [`ModuleDef`]s.
#[must_use]
pub fn builtin_module_defs() -> Vec<ModuleDef> {
    use crate::config::RegexPattern;

    vec![
        ModuleDef {
            name: "rust".to_owned(),
            when: ModuleWhen {
                files: vec!["Cargo.toml".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["rustc".to_owned(), "--version".to_owned()]),
                regex: Some(RegexPattern::new_unchecked(r"rustc\s+(\S+)".to_owned())),
            }],
            format: "v{value}".to_owned(),
            icon: Some("\u{f1617}".to_owned()),
            color: Some(Color::Red),
            connector: Some("via".to_owned()),
        },
        ModuleDef {
            name: "bun".to_owned(),
            when: ModuleWhen {
                files: vec!["bun.lockb".to_owned(), "bunfig.toml".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["bun".to_owned(), "--version".to_owned()]),
                regex: Some(RegexPattern::new_unchecked(r"(\d[\d.]*)".to_owned())),
            }],
            format: "v{value}".to_owned(),
            icon: Some("\u{e76f}".to_owned()),
            color: Some(Color::Red),
            connector: Some("via".to_owned()),
        },
        ModuleDef {
            name: "node".to_owned(),
            when: ModuleWhen {
                files: vec!["package.json".to_owned()],
                env: vec![],
            },
            source: vec![
                SourceDef {
                    env: None,
                    file: Some(".node-version".to_owned()),
                    command: None,
                    regex: None,
                },
                SourceDef {
                    env: None,
                    file: None,
                    command: Some(vec!["node".to_owned(), "--version".to_owned()]),
                    regex: Some(RegexPattern::new_unchecked(r"v?(\d[\d.]*)".to_owned())),
                },
            ],
            format: "v{value}".to_owned(),
            icon: Some("\u{e718}".to_owned()),
            color: Some(Color::Green),
            connector: Some("via".to_owned()),
        },
        ModuleDef {
            name: "go".to_owned(),
            when: ModuleWhen {
                files: vec!["go.mod".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["go".to_owned(), "version".to_owned()]),
                regex: Some(RegexPattern::new_unchecked(r"go(\d[\d.]*)".to_owned())),
            }],
            format: "v{value}".to_owned(),
            icon: Some("\u{e627}".to_owned()),
            color: Some(Color::Cyan),
            connector: Some("via".to_owned()),
        },
        ModuleDef {
            name: "python".to_owned(),
            when: ModuleWhen {
                files: vec!["pyproject.toml".to_owned(), "setup.py".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["python3".to_owned(), "--version".to_owned()]),
                regex: Some(RegexPattern::new_unchecked(r"Python\s+(\S+)".to_owned())),
            }],
            format: "v{value}".to_owned(),
            icon: Some("\u{e235}".to_owned()),
            color: Some(Color::Yellow),
            connector: Some("via".to_owned()),
        },
        ModuleDef {
            name: "ruby".to_owned(),
            when: ModuleWhen {
                files: vec!["Gemfile".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["ruby".to_owned(), "--version".to_owned()]),
                regex: Some(RegexPattern::new_unchecked(
                    r"ruby\s+(\d+\.\d+\.\d+)".to_owned(),
                )),
            }],
            format: "v{value}".to_owned(),
            icon: Some("\u{e791}".to_owned()),
            color: Some(Color::Red),
            connector: Some("via".to_owned()),
        },
    ]
}

// ---------------------------------------------------------------------------
// Merge + resolve
// ---------------------------------------------------------------------------

/// Merges built-in modules with user-defined `[[module]]` entries and compiles
/// regexes.
///
/// Order: built-in toolchains (as modules) first, then user additions.
/// Same-name entries replace in-place (preserving position).
#[must_use]
pub fn resolve_modules(user_modules: &[ModuleDef]) -> Vec<ResolvedModule> {
    let mut defs = builtin_module_defs();

    for um in user_modules {
        if let Some(existing) = defs.iter_mut().find(|d| d.name == um.name) {
            *existing = um.clone();
        } else {
            defs.push(um.clone());
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
    let style = def.color.map_or_else(
        || Style::new().fg(Color::BrightBlack),
        |c| Style::new().fg(c).bold(),
    );

    ResolvedModule {
        name: def.name,
        when: def.when,
        sources,
        format: def.format,
        icon: def.icon,
        style,
        connector: def.connector,
        speed,
    }
}

fn compile_source(def: SourceDef) -> Option<ResolvedSource> {
    let regex = def
        .regex
        .as_ref()
        .and_then(|pat| Regex::new(pat.as_str()).ok());

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

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

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
    defs.iter()
        .filter(|d| d.speed == only_speed)
        .filter(|d| check_when(&d.when, cwd, env_vars))
        .filter_map(|d| detect_module(d, cwd, env_vars, path_env))
        .collect()
}

pub(crate) fn check_when(when: &ModuleWhen, cwd: &Path, env_vars: &[(String, String)]) -> bool {
    let files_ok = when.files.is_empty() || when.files.iter().any(|f| cwd.join(f).is_file());
    let env_ok = when.env.is_empty()
        || when
            .env
            .iter()
            .any(|e| env_vars.iter().any(|(k, _)| k == e));
    files_ok && env_ok
}

pub(crate) fn detect_module(
    def: &ResolvedModule,
    cwd: &Path,
    env_vars: &[(String, String)],
    path_env: Option<&str>,
) -> Option<CustomModuleInfo> {
    // Try fast sources first (env, file)
    for source in &def.sources {
        if source.is_fast()
            && let Some(raw) = resolve_source(source, cwd, env_vars, path_env)
        {
            return Some(make_info(def, &raw));
        }
    }

    // Then try slow sources (command)
    for source in &def.sources {
        if !source.is_fast()
            && let Some(raw) = resolve_source(source, cwd, env_vars, path_env)
        {
            return Some(make_info(def, &raw));
        }
    }

    None
}

fn resolve_source(
    source: &ResolvedSource,
    cwd: &Path,
    env_vars: &[(String, String)],
    path_env: Option<&str>,
) -> Option<String> {
    match source {
        ResolvedSource::Env { name, regex } => {
            let value = env_vars.iter().find(|(k, _)| k == name).map(|(_, v)| v)?;
            apply_regex(value, regex.as_ref())
        }
        ResolvedSource::File { path, regex } => {
            let content = std::fs::read_to_string(cwd.join(path)).ok()?;
            let trimmed = content.trim();
            if trimmed.is_empty() || trimmed.contains('/') {
                return None;
            }
            apply_regex(trimmed, regex.as_ref())
        }
        ResolvedSource::Command { args, regex } => {
            let (program, cmd_args) = args.split_first()?;
            let mut command = Command::new(program);
            command.args(cmd_args).current_dir(cwd);
            if let Some(path) = path_env {
                command.env("PATH", path);
            }
            let output = command.output().ok()?;
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let trimmed = stdout.trim();
            if trimmed.is_empty() {
                return None;
            }
            apply_regex(trimmed, regex.as_ref())
        }
    }
}

fn apply_regex(input: &str, regex: Option<&Regex>) -> Option<String> {
    if let Some(re) = regex {
        let caps = re.captures(input)?;
        Some(caps.get(1)?.as_str().to_owned())
    } else {
        Some(input.to_owned())
    }
}

/// Format placeholder for value substitution in module format strings.
const VALUE_PLACEHOLDER: &str = "{value}";

fn make_info(def: &ResolvedModule, raw_value: &str) -> CustomModuleInfo {
    let value = def.format.replace(VALUE_PLACEHOLDER, raw_value);
    CustomModuleInfo {
        name: def.name.clone(),
        value,
        icon: def.icon.clone(),
        style: def.style,
        connector: def.connector.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegexPattern;

    // -- resolve_modules ------------------------------------------------------

    #[test]
    fn test_resolve_modules_builtin_only() {
        let resolved = resolve_modules(&[]);
        assert_eq!(resolved.len(), 6);
        assert_eq!(resolved[0].name, "rust");
        // Built-in toolchains are slow (they have command sources)
        assert_eq!(resolved[0].speed, ModuleSpeed::Slow);
    }

    #[test]
    fn test_resolve_modules_user_module_appended() {
        let user = vec![ModuleDef {
            name: "aws".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["AWS_PROFILE".to_owned()],
            },
            source: vec![SourceDef {
                env: Some("AWS_PROFILE".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: Some(Color::Yellow),
            connector: None,
        }];
        let resolved = resolve_modules(&user);
        assert_eq!(resolved.len(), 7);
        assert_eq!(resolved[6].name, "aws");
        assert_eq!(resolved[6].speed, ModuleSpeed::Fast);
    }

    #[test]
    fn test_resolve_modules_user_module_overrides_builtin() {
        let user = vec![ModuleDef {
            name: "rust".to_owned(),
            when: ModuleWhen {
                files: vec!["Cargo.toml".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: Some("RUST_VERSION".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: Some("R".to_owned()),
            color: Some(Color::Blue),
            connector: None,
        }];
        let resolved = resolve_modules(&user);
        assert_eq!(resolved.len(), 6, "count unchanged");
        assert_eq!(resolved[0].name, "rust", "still first");
        assert_eq!(resolved[0].icon.as_deref(), Some("R"));
        assert_eq!(resolved[0].speed, ModuleSpeed::Fast);
    }

    #[test]
    fn test_resolve_modules_user_module_with_command() {
        let user = vec![ModuleDef {
            name: "zig".to_owned(),
            when: ModuleWhen {
                files: vec!["build.zig".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["zig".to_owned(), "version".to_owned()]),
                regex: Some(RegexPattern::new_unchecked(r"(\d[\d.]*)".to_owned())),
            }],
            format: "v{value}".to_owned(),
            icon: Some("Z".to_owned()),
            color: Some(Color::Yellow),
            connector: Some("via".to_owned()),
        }];
        let resolved = resolve_modules(&user);
        assert_eq!(resolved.len(), 7);
        assert_eq!(resolved[6].name, "zig");
        assert_eq!(resolved[6].connector.as_deref(), Some("via"));
        assert_eq!(resolved[6].format, "v{value}");
        assert_eq!(resolved[6].speed, ModuleSpeed::Slow);
    }

    #[test]
    fn test_resolve_modules_speed_fast_only_env() {
        let user = vec![ModuleDef {
            name: "env_only".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                env: Some("FOO".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }];
        let resolved = resolve_modules(&user);
        let m = resolved.iter().find(|r| r.name == "env_only");
        assert_eq!(m.map(|m| m.speed), Some(ModuleSpeed::Fast));
    }

    #[test]
    fn test_resolve_modules_speed_slow_with_command() {
        let user = vec![ModuleDef {
            name: "mixed".to_owned(),
            when: ModuleWhen::default(),
            source: vec![
                SourceDef {
                    env: Some("FOO".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                },
                SourceDef {
                    env: None,
                    file: None,
                    command: Some(vec!["echo".to_owned(), "bar".to_owned()]),
                    regex: None,
                },
            ],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }];
        let resolved = resolve_modules(&user);
        let m = resolved.iter().find(|r| r.name == "mixed");
        assert_eq!(m.map(|m| m.speed), Some(ModuleSpeed::Slow));
    }

    // -- detect_modules -------------------------------------------------------

    #[test]
    fn test_detect_env_source() {
        let defs = resolve_modules(&[ModuleDef {
            name: "aws".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["AWS_PROFILE".to_owned()],
            },
            source: vec![SourceDef {
                env: Some("AWS_PROFILE".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: Some(Color::Yellow),
            connector: None,
        }]);

        let env_vars = vec![("AWS_PROFILE".to_owned(), "production".to_owned())];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);

        let aws = results.iter().find(|r| r.name == "aws");
        assert!(aws.is_some(), "aws module should be detected");
        assert_eq!(aws.map(|a| a.value.as_str()), Some("production"));
    }

    #[test]
    fn test_detect_env_source_not_set() {
        let defs = resolve_modules(&[ModuleDef {
            name: "aws".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["AWS_PROFILE".to_owned()],
            },
            source: vec![SourceDef {
                env: Some("AWS_PROFILE".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let results = detect_modules(&defs, Path::new("/tmp"), &[], None, ModuleSpeed::Fast);
        assert!(
            results.iter().all(|r| r.name != "aws"),
            "aws should not be detected without env var"
        );
    }

    #[test]
    fn test_detect_file_source() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".tool-versions"), "erlang 26.0\n")?;
        std::fs::write(dir.path().join("rebar.config"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "erlang".to_owned(),
            when: ModuleWhen {
                files: vec!["rebar.config".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: Some(".tool-versions".to_owned()),
                command: None,
                regex: Some(RegexPattern::new_unchecked(r"erlang\s+(\S+)".to_owned())),
            }],
            format: "v{value}".to_owned(),
            icon: None,
            color: None,
            connector: Some("via".to_owned()),
        }]);

        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Fast);
        let erlang = results.iter().find(|r| r.name == "erlang");
        assert!(erlang.is_some(), "erlang module should be detected");
        assert_eq!(erlang.map(|e| e.value.as_str()), Some("v26.0"));
        Ok(())
    }

    #[test]
    fn test_detect_command_source() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("build.zig"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "echo_ver".to_owned(),
            when: ModuleWhen {
                files: vec!["build.zig".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["echo".to_owned(), "1.2.3".to_owned()]),
                regex: None,
            }],
            format: "v{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);
        let m = results.iter().find(|r| r.name == "echo_ver");
        assert!(m.is_some(), "echo_ver should be detected");
        assert_eq!(m.map(|e| e.value.as_str()), Some("v1.2.3"));
        Ok(())
    }

    #[test]
    fn test_detect_fast_source_preferred_over_command() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "mixed".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![
                SourceDef {
                    env: Some("MY_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                },
                SourceDef {
                    env: None,
                    file: None,
                    command: Some(vec!["echo".to_owned(), "from-cmd".to_owned()]),
                    regex: None,
                },
            ],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let env_vars = vec![("MY_VERSION".to_owned(), "from-env".to_owned())];
        // Even though this is a slow module (has command), env source resolves first
        let results = detect_modules(&defs, dir.path(), &env_vars, None, ModuleSpeed::Slow);
        let m = results.iter().find(|r| r.name == "mixed");
        assert_eq!(
            m.map(|m| m.value.as_str()),
            Some("from-env"),
            "env source should be preferred"
        );
        Ok(())
    }

    #[test]
    fn test_detect_format_string() {
        let defs = resolve_modules(&[ModuleDef {
            name: "test".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["FOO".to_owned()],
            },
            source: vec![SourceDef {
                env: Some("FOO".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "v{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let env_vars = vec![("FOO".to_owned(), "1.0".to_owned())];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);
        let m = results.iter().find(|r| r.name == "test");
        assert_eq!(m.map(|m| m.value.as_str()), Some("v1.0"));
    }

    #[test]
    fn test_detect_regex_on_env_source() {
        let defs = resolve_modules(&[ModuleDef {
            name: "test".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["VERSION_STR".to_owned()],
            },
            source: vec![SourceDef {
                env: Some("VERSION_STR".to_owned()),
                file: None,
                command: None,
                regex: Some(RegexPattern::new_unchecked(r"v(\d+\.\d+)".to_owned())),
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let env_vars = vec![("VERSION_STR".to_owned(), "v1.23.456-beta".to_owned())];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);
        let m = results.iter().find(|r| r.name == "test");
        assert_eq!(m.map(|m| m.value.as_str()), Some("1.23"));
    }

    #[test]
    fn test_detect_when_files_not_present() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        // No marker file
        let defs = resolve_modules(&[ModuleDef {
            name: "test".to_owned(),
            when: ModuleWhen {
                files: vec!["missing.txt".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: Some("FOO".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let env_vars = vec![("FOO".to_owned(), "bar".to_owned())];
        let results = detect_modules(&defs, dir.path(), &env_vars, None, ModuleSpeed::Fast);
        assert!(
            results.is_empty(),
            "module should not trigger without marker file"
        );
        Ok(())
    }

    #[test]
    fn test_detect_when_empty_always_triggers() {
        let defs = resolve_modules(&[ModuleDef {
            name: "always".to_owned(),
            when: ModuleWhen::default(), // empty when
            source: vec![SourceDef {
                env: Some("FOO".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let env_vars = vec![("FOO".to_owned(), "bar".to_owned())];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);
        assert_eq!(results.len(), 1, "empty when should always trigger");
    }

    #[test]
    fn test_detect_command_failure_returns_none() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "failing".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["false".to_owned()]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            color: None,
            connector: None,
        }]);

        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);
        assert!(
            results.is_empty(),
            "failing command should produce no output"
        );
        Ok(())
    }

    #[test]
    fn test_detect_toolchain_compat_via_modules() -> Result<(), Box<dyn std::error::Error>> {
        // Built-in toolchains should still detect when marker files exist
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("Cargo.toml"), "")?;

        let defs = resolve_modules(&[]);
        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);
        // May or may not detect (depends on rustc being available),
        // but no panic and at most 1 rust entry
        assert!(results.len() <= 1);
        if let Some(tc) = results.first() {
            assert_eq!(tc.name, "rust");
            assert!(
                tc.value.starts_with('v'),
                "should have v prefix: {}",
                tc.value
            );
            assert_eq!(tc.connector.as_deref(), Some("via"));
        }
        Ok(())
    }
}
