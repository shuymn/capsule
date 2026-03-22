//! Custom prompt modules — user-defined segments via `[[module]]` config DSL.
//!
//! Each [`ResolvedModule`] defines trigger conditions, value sources, and
//! display metadata. Sources are tried in order: fast sources (env, file)
//! first, then slow sources (command) if fast sources all fail.

mod builtins;
mod compile;
mod detect;
mod facts;

use std::path::PathBuf;

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
    /// Optional arbitration rule for collapsing competing detected modules.
    pub arbitration: Option<Arbitration>,
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
#[derive(Debug, Clone)]
pub(crate) struct RequestFacts {
    cwd: PathBuf,
    env_vars: Vec<(String, String)>,
    path_env: Option<String>,
    read_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModuleDependencyInputs {
    pub(crate) env_vars: Vec<String>,
    pub(crate) files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        builtins::{
            BUN_ARBITRATION_PRIORITY, JS_RUNTIME_ARBITRATION_GROUP, NODE_ARBITRATION_PRIORITY,
        },
        *,
    };
    use crate::{
        config::{ModuleDef, RegexPattern, SourceDef, StyleConfig},
        render::style::Color,
    };

    fn arbitration(group: &str, priority: u32) -> Arbitration {
        Arbitration {
            group: group.to_owned(),
            priority,
        }
    }

    fn js_runtime_arbitration(priority: u32) -> Arbitration {
        arbitration(JS_RUNTIME_ARBITRATION_GROUP, priority)
    }

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
    fn test_resolve_modules_builtin_arbitration_for_js_runtimes() {
        let resolved = resolve_modules(&[]);
        let bun = resolved.iter().find(|module| module.name == "bun");
        let node = resolved.iter().find(|module| module.name == "node");

        assert_eq!(
            bun.and_then(|module| module.arbitration.as_ref()),
            Some(&js_runtime_arbitration(BUN_ARBITRATION_PRIORITY))
        );
        assert_eq!(
            node.and_then(|module| module.arbitration.as_ref()),
            Some(&js_runtime_arbitration(NODE_ARBITRATION_PRIORITY))
        );
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
            style: StyleConfig {
                fg: Some(Color::Yellow),
                bold: None,
                dimmed: None,
            },
            connector: None,
            arbitration: None,
        }];
        let resolved = resolve_modules(&user);
        assert_eq!(resolved.len(), 7);
        assert_eq!(resolved[6].name, "aws");
        assert_eq!(resolved[6].speed, ModuleSpeed::Fast);
        assert_eq!(resolved[6].arbitration, None);
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
            style: StyleConfig {
                fg: Some(Color::Blue),
                bold: None,
                dimmed: None,
            },
            connector: None,
            arbitration: None,
        }];
        let resolved = resolve_modules(&user);
        assert_eq!(resolved.len(), 6, "count unchanged");
        assert_eq!(resolved[0].name, "rust", "still first");
        assert_eq!(resolved[0].icon.as_deref(), Some("R"));
        assert_eq!(resolved[0].speed, ModuleSpeed::Fast);
        assert_eq!(resolved[0].arbitration, None);
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
            style: StyleConfig {
                fg: Some(Color::Yellow),
                bold: None,
                dimmed: None,
            },
            connector: Some("via".to_owned()),
            arbitration: None,
        }];
        let resolved = resolve_modules(&user);
        assert_eq!(resolved.len(), 7);
        assert_eq!(resolved[6].name, "zig");
        assert_eq!(resolved[6].connector.as_deref(), Some("via"));
        assert_eq!(resolved[6].format, "v{value}");
        assert_eq!(resolved[6].speed, ModuleSpeed::Slow);
    }

    #[test]
    fn test_resolve_modules_user_module_keeps_arbitration() {
        let user = vec![ModuleDef {
            name: "deno".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                env: Some("DENO_VERSION".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: Some(arbitration("javascript", 30)),
        }];

        let resolved = resolve_modules(&user);
        let deno = resolved.iter().find(|module| module.name == "deno");
        assert_eq!(
            deno.and_then(|module| module.arbitration.as_ref()),
            Some(&arbitration("javascript", 30))
        );
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }];
        let resolved = resolve_modules(&user);
        let m = resolved.iter().find(|r| r.name == "mixed");
        assert_eq!(m.map(|m| m.speed), Some(ModuleSpeed::Slow));
    }

    #[test]
    fn test_resolve_modules_empty_command_args_filtered() {
        let defs = resolve_modules(&[ModuleDef {
            name: "empty_cmd".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec![]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let m = defs.iter().find(|r| r.name == "empty_cmd");
        assert!(
            m.is_some_and(|m| m.sources.is_empty()),
            "empty command args must be filtered during compilation"
        );
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
            style: StyleConfig {
                fg: Some(Color::Yellow),
                bold: None,
                dimmed: None,
            },
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: Some("via".to_owned()),
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
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
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);
        assert!(
            results.is_empty(),
            "failing command should produce no output"
        );
        Ok(())
    }

    #[test]
    fn test_detect_modules_builtin_node_when_package_json_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        std::fs::write(dir.path().join(".node-version"), "22.0.0\n")?;

        let defs = resolve_modules(&[]);
        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);

        assert_eq!(results.len(), 1, "only node should be detected");
        assert_eq!(results[0].name, "node");
        assert_eq!(results[0].value, "v22.0.0");
        Ok(())
    }

    #[test]
    fn test_detect_modules_builtin_bun_wins_over_node_in_same_group()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        std::fs::write(dir.path().join(".node-version"), "22.0.0\n")?;
        std::fs::write(dir.path().join("bun.lock"), "")?;
        std::fs::write(dir.path().join(".bun-version"), "1.2.3\n")?;

        let defs = resolve_modules(&[]);
        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);

        assert_eq!(results.len(), 1, "bun should win arbitration over node");
        assert_eq!(results[0].name, "bun");
        assert_eq!(results[0].value, "v1.2.3");
        Ok(())
    }

    #[test]
    fn test_detect_modules_same_group_keeps_lower_priority_user_module() {
        let defs = resolve_modules(&[
            ModuleDef {
                name: "alpha".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("ALPHA_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: Some(arbitration("runtime", 20)),
            },
            ModuleDef {
                name: "beta".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("BETA_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: Some(arbitration("runtime", 10)),
            },
        ]);

        let env_vars = vec![
            ("ALPHA_VERSION".to_owned(), "1.0.0".to_owned()),
            ("BETA_VERSION".to_owned(), "2.0.0".to_owned()),
        ];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);

        assert_eq!(
            results.len(),
            1,
            "only the lower-priority module should remain"
        );
        assert_eq!(results[0].name, "beta");
    }

    #[test]
    fn test_detect_modules_same_group_equal_priority_keeps_earlier_definition() {
        let defs = resolve_modules(&[
            ModuleDef {
                name: "first".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("FIRST_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: Some(arbitration("runtime", 10)),
            },
            ModuleDef {
                name: "second".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("SECOND_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: Some(arbitration("runtime", 10)),
            },
        ]);

        let env_vars = vec![
            ("FIRST_VERSION".to_owned(), "1.0.0".to_owned()),
            ("SECOND_VERSION".to_owned(), "2.0.0".to_owned()),
        ];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);

        assert_eq!(
            results.len(),
            1,
            "equal priority should keep the earlier module"
        );
        assert_eq!(results[0].name, "first");
    }

    #[test]
    fn test_detect_modules_without_arbitration_are_unaffected() {
        let defs = resolve_modules(&[
            ModuleDef {
                name: "winner".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("WINNER_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: Some(arbitration("runtime", 10)),
            },
            ModuleDef {
                name: "plain".to_owned(),
                when: ModuleWhen::default(),
                source: vec![SourceDef {
                    env: Some("PLAIN_VERSION".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                }],
                format: "{value}".to_owned(),
                icon: None,
                style: StyleConfig::default(),
                connector: None,
                arbitration: None,
            },
        ]);

        let env_vars = vec![
            ("WINNER_VERSION".to_owned(), "1.0.0".to_owned()),
            ("PLAIN_VERSION".to_owned(), "2.0.0".to_owned()),
        ];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "winner");
        assert_eq!(results[1].name, "plain");
    }

    #[test]
    fn test_request_facts_matching_dependency_inputs_only_include_matching_modules()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("package.json"), "{}")?;
        std::fs::write(dir.path().join(".node-version"), "22.0.0\n")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "terraform".to_owned(),
            when: ModuleWhen {
                files: vec!["main.tf".to_owned()],
                env: vec!["TF_WORKSPACE".to_owned()],
            },
            source: vec![
                SourceDef {
                    env: Some("TF_WORKSPACE".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                },
                SourceDef {
                    env: None,
                    file: Some(".terraform-version".to_owned()),
                    command: None,
                    regex: None,
                },
            ],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let facts = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
        let inputs = facts.matching_dependency_inputs(&defs, ModuleSpeed::Slow);

        assert_eq!(inputs.env_vars, Vec::<String>::new());
        assert_eq!(
            inputs.files,
            vec!["package.json".to_owned(), ".node-version".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn test_request_facts_detect_module_uses_forwarded_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "tool".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["fake-tool".to_owned(), "--version".to_owned()]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);
        let module = defs.iter().find(|resolved| resolved.name == "tool");
        let Some(module) = module else {
            return Err("tool module missing".into());
        };

        let bin_dir = tempfile::tempdir()?;
        let script_path = bin_dir.path().join("fake-tool");
        std::fs::write(&script_path, "#!/bin/sh\necho forwarded\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&script_path)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions)?;
        }

        let facts = RequestFacts::collect(
            dir.path().to_path_buf(),
            vec![(
                "PATH".to_owned(),
                bin_dir.path().to_string_lossy().into_owned(),
            )],
        )
        .with_forwarded_path_env();

        let detected = facts.detect_module(module);
        assert_eq!(
            detected.as_ref().map(|info| info.value.as_str()),
            Some("forwarded")
        );
        Ok(())
    }

    #[test]
    fn test_detect_modules_does_not_treat_forwarded_path_env_as_override()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "tool".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec!["fake-tool".to_owned(), "--version".to_owned()]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let bin_dir = tempfile::tempdir()?;
        let script_path = bin_dir.path().join("fake-tool");
        std::fs::write(&script_path, "#!/bin/sh\necho forwarded\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&script_path)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions)?;
        }

        let results = detect_modules(
            &defs,
            dir.path(),
            &[(
                "PATH".to_owned(),
                bin_dir.path().to_string_lossy().into_owned(),
            )],
            None,
            ModuleSpeed::Slow,
        );
        assert!(
            results.is_empty(),
            "PATH in env_vars alone must not change detect_modules command lookup"
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

    #[test]
    fn test_detect_empty_env_var_value() {
        let defs = resolve_modules(&[ModuleDef {
            name: "empty_env".to_owned(),
            when: ModuleWhen {
                files: vec![],
                env: vec!["EMPTY_VAR".to_owned()],
            },
            source: vec![SourceDef {
                env: Some("EMPTY_VAR".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let env_vars = vec![("EMPTY_VAR".to_owned(), String::new())];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);
        let m = results.iter().find(|r| r.name == "empty_env");
        assert!(
            m.is_some(),
            "empty env var value should still trigger detection"
        );
        assert_eq!(m.map(|m| m.value.as_str()), Some(""));
    }

    #[test]
    fn test_detect_empty_file_content_filtered() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;
        std::fs::write(dir.path().join(".version"), "")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "empty_file".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: Some(".version".to_owned()),
                command: None,
                regex: None,
            }],
            format: "v{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Fast);
        assert!(
            results.iter().all(|r| r.name != "empty_file"),
            "empty file content must not produce a detection"
        );
        Ok(())
    }

    #[test]
    fn test_detect_file_source_path_traversal_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub)?;
        std::fs::write(sub.join("marker"), "")?;
        std::fs::write(dir.path().join("evil"), "/bin/bad\n")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "traversal".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: Some("../evil".to_owned()),
                command: None,
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let results = detect_modules(&defs, &sub, &[], None, ModuleSpeed::Fast);
        assert!(
            results.iter().all(|r| r.name != "traversal"),
            "file source with path traversal ('..') must be rejected"
        );
        Ok(())
    }

    #[test]
    fn test_detect_format_no_recursive_expansion() {
        let defs = resolve_modules(&[ModuleDef {
            name: "format_inject".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                env: Some("INJECT_VAR".to_owned()),
                file: None,
                command: None,
                regex: None,
            }],
            format: "prefix-{value}-suffix".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let env_vars = vec![("INJECT_VAR".to_owned(), "{value}".to_owned())];
        let results = detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast);
        let m = results.iter().find(|r| r.name == "format_inject");
        assert_eq!(
            m.map(|m| m.value.as_str()),
            Some("prefix-{value}-suffix"),
            "{{value}} in raw value must not be recursively expanded"
        );
    }

    #[test]
    fn test_detect_command_no_shell_injection() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;
        let sentinel = dir.path().join("pwned");

        let defs = resolve_modules(&[ModuleDef {
            name: "shell_inject".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: None,
                command: Some(vec![
                    "echo".to_owned(),
                    format!("safe; touch {}", sentinel.display()),
                ]),
                regex: None,
            }],
            format: "{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let _results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow);
        assert!(
            !sentinel.exists(),
            "shell metacharacters in command args must not be interpreted"
        );
        Ok(())
    }

    #[test]
    fn test_detect_concurrent_no_corruption() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("marker"), "")?;
        std::fs::write(dir.path().join(".version"), "1.0.0\n")?;

        let defs = resolve_modules(&[ModuleDef {
            name: "concurrent".to_owned(),
            when: ModuleWhen {
                files: vec!["marker".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                env: None,
                file: Some(".version".to_owned()),
                command: None,
                regex: None,
            }],
            format: "v{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
        }]);

        let defs = std::sync::Arc::new(defs);
        let dir_path = dir.path().to_path_buf();
        let mut handles = Vec::new();

        for _ in 0..8 {
            let defs = std::sync::Arc::clone(&defs);
            let path = dir_path.clone();
            handles.push(std::thread::spawn(move || {
                detect_modules(&defs, &path, &[], None, ModuleSpeed::Fast)
            }));
        }

        for handle in handles {
            let results = handle
                .join()
                .map_err(|panic_payload| format!("thread panicked: {panic_payload:?}"))?;
            let m = results.iter().find(|r| r.name == "concurrent");
            assert!(m.is_some(), "each thread must detect the module");
            assert_eq!(
                m.map(|m| m.value.as_str()),
                Some("v1.0.0"),
                "value must be consistent across threads"
            );
        }
        Ok(())
    }

    // -- compute_dep_hash -----------------------------------------------------

    #[test]
    fn test_dep_hash_empty_inputs_is_deterministic() {
        let facts = RequestFacts::collect(PathBuf::from("/tmp"), vec![]);
        let inputs = ModuleDependencyInputs::default();
        let h1 = inputs.compute_dep_hash(&facts);
        let h2 = inputs.compute_dep_hash(&facts);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_dep_hash_same_env_values_produce_same_hash() {
        let facts = RequestFacts::collect(
            PathBuf::from("/tmp"),
            vec![("MY_VAR".to_owned(), "val".to_owned())],
        );
        let mut inputs = ModuleDependencyInputs::default();
        inputs.env_vars.push("MY_VAR".to_owned());
        let h1 = inputs.compute_dep_hash(&facts);
        let h2 = inputs.compute_dep_hash(&facts);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_dep_hash_different_env_values_produce_different_hash() {
        let facts_a = RequestFacts::collect(
            PathBuf::from("/tmp"),
            vec![("MY_VAR".to_owned(), "a".to_owned())],
        );
        let facts_b = RequestFacts::collect(
            PathBuf::from("/tmp"),
            vec![("MY_VAR".to_owned(), "b".to_owned())],
        );
        let mut inputs = ModuleDependencyInputs::default();
        inputs.env_vars.push("MY_VAR".to_owned());
        assert_ne!(
            inputs.compute_dep_hash(&facts_a),
            inputs.compute_dep_hash(&facts_b),
        );
    }

    #[test]
    fn test_dep_hash_env_present_vs_absent_produce_different_hash() {
        let facts_present = RequestFacts::collect(
            PathBuf::from("/tmp"),
            vec![("MY_VAR".to_owned(), "x".to_owned())],
        );
        let facts_absent = RequestFacts::collect(PathBuf::from("/tmp"), vec![]);
        let mut inputs = ModuleDependencyInputs::default();
        inputs.env_vars.push("MY_VAR".to_owned());
        assert_ne!(
            inputs.compute_dep_hash(&facts_present),
            inputs.compute_dep_hash(&facts_absent),
        );
    }

    #[test]
    fn test_dep_hash_file_existence_changes_hash() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut inputs = ModuleDependencyInputs::default();
        inputs.files.push("marker".to_owned());

        let facts_no_file = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
        let h_without = inputs.compute_dep_hash(&facts_no_file);

        std::fs::write(dir.path().join("marker"), "")?;
        let facts_with_file = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
        let h_with = inputs.compute_dep_hash(&facts_with_file);

        assert_ne!(h_without, h_with);
        Ok(())
    }

    #[test]
    fn test_dep_hash_insertion_order_does_not_affect_result() {
        let facts = RequestFacts::collect(
            PathBuf::from("/tmp"),
            vec![
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "2".to_owned()),
            ],
        );

        let mut inputs_ab = ModuleDependencyInputs::default();
        inputs_ab.env_vars.push("A".to_owned());
        inputs_ab.env_vars.push("B".to_owned());

        let mut inputs_ba = ModuleDependencyInputs::default();
        inputs_ba.env_vars.push("B".to_owned());
        inputs_ba.env_vars.push("A".to_owned());

        assert_eq!(
            inputs_ab.compute_dep_hash(&facts),
            inputs_ba.compute_dep_hash(&facts),
        );
    }
}
