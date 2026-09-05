use std::path::{Path, PathBuf};

use super::*;
use crate::{
    config::{ModuleDef, ModuleSlot, RegexPattern, SourceDef, StyleConfig},
    render::style::Color,
};

fn arbitration(group: &str, priority: u32) -> Arbitration {
    Arbitration {
        group: group.to_owned(),
        priority,
    }
}

// -- resolve_modules ------------------------------------------------------

#[test]
fn resolve_modules_empty() {
    let resolved = resolve_modules(&[]);
    assert!(matches!(resolved.as_slice(), []));
}

#[test]
fn resolve_modules_single() {
    let user = vec![ModuleDef {
        name: "aws".to_owned(),
        when: ModuleWhen {
            files: vec![],
            env: vec!["AWS_PROFILE".to_owned()],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
            env: Some("AWS_PROFILE".to_owned()),
            file: None,
            command: None,
            regex: None,
        }],
        format: "{value}".to_owned(),
        icon: None,
        style: StyleConfig::fg(Color::Yellow),
        connector: None,
        arbitration: None,
        slot: ModuleSlot::default(),
    }];
    let resolved = resolve_modules(&user);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].name, "aws");
    assert_eq!(resolved[0].speed, ModuleSpeed::Fast);
    assert_eq!(resolved[0].arbitration, None);
}

#[test]
fn resolve_modules_with_command() {
    let user = vec![ModuleDef {
        name: "zig".to_owned(),
        when: ModuleWhen {
            files: vec!["build.zig".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
            env: None,
            file: None,
            command: Some(vec!["zig".to_owned(), "version".to_owned()]),
            regex: Some(RegexPattern::new_unchecked(r"(\d[\d.]*)".to_owned())),
        }],
        format: "v{value}".to_owned(),
        icon: Some("Z".to_owned()),
        style: StyleConfig::fg(Color::Yellow),
        connector: Some("via".to_owned()),
        arbitration: None,
        slot: ModuleSlot::default(),
    }];
    let resolved = resolve_modules(&user);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].name, "zig");
    assert_eq!(resolved[0].connector.as_deref(), Some("via"));
    assert_eq!(
        resolved[0].format_segments,
        vec![
            detect::FormatSegment::Literal("v".to_owned()),
            detect::FormatSegment::Variable("value".to_owned()),
        ]
    );
    assert_eq!(resolved[0].speed, ModuleSpeed::Slow);
}

#[test]
fn resolve_modules_keeps_arbitration() {
    let user = vec![ModuleDef {
        name: "deno".to_owned(),
        when: ModuleWhen::default(),
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }];

    let resolved = resolve_modules(&user);
    let deno = resolved.iter().find(|module| module.name == "deno");
    assert_eq!(
        deno.and_then(|module| module.arbitration.as_ref()),
        Some(&arbitration("javascript", 30))
    );
}

#[test]
fn resolve_modules_fast_env() {
    let user = vec![ModuleDef {
        name: "env_only".to_owned(),
        when: ModuleWhen::default(),
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }];
    let resolved = resolve_modules(&user);
    let m = resolved.iter().find(|r| r.name == "env_only");
    assert_eq!(m.map(|m| m.speed), Some(ModuleSpeed::Fast));
}

#[test]
fn resolve_modules_slow_command() {
    let user = vec![ModuleDef {
        name: "mixed".to_owned(),
        when: ModuleWhen::default(),
        source: vec![
            SourceDef {
                name: "value".to_owned(),
                env: Some("FOO".to_owned()),
                file: None,
                command: None,
                regex: None,
            },
            SourceDef {
                name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }];
    let resolved = resolve_modules(&user);
    let m = resolved.iter().find(|r| r.name == "mixed");
    assert_eq!(m.map(|m| m.speed), Some(ModuleSpeed::Slow));
}

#[test]
fn resolve_modules_filters_empty_command() {
    let defs = resolve_modules(&[ModuleDef {
        name: "empty_cmd".to_owned(),
        when: ModuleWhen::default(),
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let m = defs.iter().find(|r| r.name == "empty_cmd");
    assert!(
        m.is_some_and(|m| m.source_groups.is_empty()),
        "empty command args must be filtered during compilation"
    );
}

// -- detect_modules -------------------------------------------------------

#[tokio::test]
async fn detect_env_source() {
    let defs = resolve_modules(&[ModuleDef {
        name: "aws".to_owned(),
        when: ModuleWhen {
            files: vec![],
            env: vec!["AWS_PROFILE".to_owned()],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
            env: Some("AWS_PROFILE".to_owned()),
            file: None,
            command: None,
            regex: None,
        }],
        format: "{value}".to_owned(),
        icon: None,
        style: StyleConfig::fg(Color::Yellow),
        connector: None,
        arbitration: None,
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("AWS_PROFILE".to_owned(), "production".to_owned())];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;

    let aws = results.iter().find(|r| r.name == "aws");
    assert!(aws.is_some(), "aws module should be detected");
    assert_eq!(aws.map(|a| a.value.as_str()), Some("production"));
}

#[tokio::test]
async fn detect_env_source_missing() {
    let defs = resolve_modules(&[ModuleDef {
        name: "aws".to_owned(),
        when: ModuleWhen {
            files: vec![],
            env: vec!["AWS_PROFILE".to_owned()],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let results = detect_modules(&defs, Path::new("/tmp"), &[], None, ModuleSpeed::Fast).await;
    assert!(
        results.iter().all(|r| r.name != "aws"),
        "aws should not be detected without env var"
    );
}

#[tokio::test]
async fn detect_file_source() -> Result<(), Box<dyn std::error::Error>> {
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
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Fast).await;
    let erlang = results.iter().find(|r| r.name == "erlang");
    assert!(erlang.is_some(), "erlang module should be detected");
    assert_eq!(erlang.map(|e| e.value.as_str()), Some("v26.0"));
    Ok(())
}

#[tokio::test]
async fn detect_command_source() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("build.zig"), "")?;

    let defs = resolve_modules(&[ModuleDef {
        name: "echo_ver".to_owned(),
        when: ModuleWhen {
            files: vec!["build.zig".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow).await;
    let m = results.iter().find(|r| r.name == "echo_ver");
    assert!(m.is_some(), "echo_ver should be detected");
    assert_eq!(m.map(|e| e.value.as_str()), Some("v1.2.3"));
    Ok(())
}

#[tokio::test]
async fn detect_fast_source_preferred() -> Result<(), Box<dyn std::error::Error>> {
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
                name: "value".to_owned(),
                env: Some("MY_VERSION".to_owned()),
                file: None,
                command: None,
                regex: None,
            },
            SourceDef {
                name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("MY_VERSION".to_owned(), "from-env".to_owned())];
    // Even though this is a slow module (has command), env source resolves first
    let results = detect_modules(&defs, dir.path(), &env_vars, None, ModuleSpeed::Slow).await;
    let m = results.iter().find(|r| r.name == "mixed");
    assert_eq!(
        m.map(|m| m.value.as_str()),
        Some("from-env"),
        "env source should be preferred"
    );
    Ok(())
}

#[tokio::test]
async fn detect_format_string() {
    let defs = resolve_modules(&[ModuleDef {
        name: "test".to_owned(),
        when: ModuleWhen {
            files: vec![],
            env: vec!["FOO".to_owned()],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("FOO".to_owned(), "1.0".to_owned())];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;
    let m = results.iter().find(|r| r.name == "test");
    assert_eq!(m.map(|m| m.value.as_str()), Some("v1.0"));
}

#[tokio::test]
async fn detect_env_regex() {
    let defs = resolve_modules(&[ModuleDef {
        name: "test".to_owned(),
        when: ModuleWhen {
            files: vec![],
            env: vec!["VERSION_STR".to_owned()],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("VERSION_STR".to_owned(), "v1.23.456-beta".to_owned())];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;
    let m = results.iter().find(|r| r.name == "test");
    assert_eq!(m.map(|m| m.value.as_str()), Some("1.23"));
}

#[tokio::test]
async fn detect_when_missing_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    // No marker file
    let defs = resolve_modules(&[ModuleDef {
        name: "test".to_owned(),
        when: ModuleWhen {
            files: vec!["missing.txt".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("FOO".to_owned(), "bar".to_owned())];
    let results = detect_modules(&defs, dir.path(), &env_vars, None, ModuleSpeed::Fast).await;
    assert!(
        results.is_empty(),
        "module should not trigger without marker file"
    );
    Ok(())
}

#[tokio::test]
async fn detect_when_empty() {
    let defs = resolve_modules(&[ModuleDef {
        name: "always".to_owned(),
        when: ModuleWhen::default(), // empty when
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("FOO".to_owned(), "bar".to_owned())];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;
    assert_eq!(results.len(), 1, "empty when should always trigger");
}

#[tokio::test]
async fn detect_command_failure() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("marker"), "")?;

    let defs = resolve_modules(&[ModuleDef {
        name: "failing".to_owned(),
        when: ModuleWhen {
            files: vec!["marker".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow).await;
    assert!(
        results.is_empty(),
        "failing command should produce no output"
    );
    Ok(())
}

#[tokio::test]
async fn detect_same_group_keeps_lower_priority() {
    let defs = resolve_modules(&[
        ModuleDef {
            name: "alpha".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
        ModuleDef {
            name: "beta".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
    ]);

    let env_vars = vec![
        ("ALPHA_VERSION".to_owned(), "1.0.0".to_owned()),
        ("BETA_VERSION".to_owned(), "2.0.0".to_owned()),
    ];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;

    assert_eq!(
        results.len(),
        1,
        "only the lower-priority module should remain"
    );
    assert_eq!(results[0].name, "beta");
}

#[tokio::test]
async fn detect_same_group_keeps_earlier_definition() {
    let defs = resolve_modules(&[
        ModuleDef {
            name: "first".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
        ModuleDef {
            name: "second".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
    ]);

    let env_vars = vec![
        ("FIRST_VERSION".to_owned(), "1.0.0".to_owned()),
        ("SECOND_VERSION".to_owned(), "2.0.0".to_owned()),
    ];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;

    assert_eq!(
        results.len(),
        1,
        "equal priority should keep the earlier module"
    );
    assert_eq!(results[0].name, "first");
}

#[tokio::test]
async fn detect_without_arbitration() {
    let defs = resolve_modules(&[
        ModuleDef {
            name: "winner".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
        ModuleDef {
            name: "plain".to_owned(),
            when: ModuleWhen::default(),
            source: vec![SourceDef {
                name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
    ]);

    let env_vars = vec![
        ("WINNER_VERSION".to_owned(), "1.0.0".to_owned()),
        ("PLAIN_VERSION".to_owned(), "2.0.0".to_owned()),
    ];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].name, "winner");
    assert_eq!(results[1].name, "plain");
}

#[test]
fn request_facts_matching_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    // Create marker files for the "node" module but not for "terraform"
    std::fs::write(dir.path().join("package.json"), "{}")?;
    std::fs::write(dir.path().join(".node-version"), "22.0.0\n")?;

    let defs = resolve_modules(&[
        ModuleDef {
            name: "node".to_owned(),
            when: ModuleWhen {
                files: vec!["package.json".to_owned()],
                env: vec![],
            },
            source: vec![SourceDef {
                name: "value".to_owned(),
                env: None,
                file: Some(".node-version".to_owned()),
                command: None,
                regex: None,
            }],
            format: "v{value}".to_owned(),
            icon: None,
            style: StyleConfig::default(),
            connector: None,
            arbitration: None,
            slot: ModuleSlot::default(),
        },
        ModuleDef {
            name: "terraform".to_owned(),
            when: ModuleWhen {
                files: vec!["main.tf".to_owned()],
                env: vec!["TF_WORKSPACE".to_owned()],
            },
            source: vec![
                SourceDef {
                    name: "value".to_owned(),
                    env: Some("TF_WORKSPACE".to_owned()),
                    file: None,
                    command: None,
                    regex: None,
                },
                SourceDef {
                    name: "value".to_owned(),
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
            slot: ModuleSlot::default(),
        },
    ]);

    let facts = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let inputs = facts.matching_dependency_inputs(&defs, ModuleSpeed::Fast);

    assert_eq!(inputs.env_vars, Vec::<String>::new());
    assert_eq!(inputs.trigger_files, vec!["package.json".to_owned()]);
    assert_eq!(inputs.source_files, vec![".node-version".to_owned()]);
    Ok(())
}

#[tokio::test]
async fn request_facts_uses_forwarded_path() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("marker"), "")?;

    let defs = resolve_modules(&[ModuleDef {
        name: "tool".to_owned(),
        when: ModuleWhen {
            files: vec!["marker".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
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

    let detected = facts.detect_module(module).await;
    assert_eq!(
        detected.as_ref().map(|info| info.value.as_str()),
        Some("forwarded")
    );
    Ok(())
}

#[tokio::test]
async fn detect_ignores_forwarded_path_env() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("marker"), "")?;

    let defs = resolve_modules(&[ModuleDef {
        name: "tool".to_owned(),
        when: ModuleWhen {
            files: vec!["marker".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
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
    )
    .await;
    assert!(
        results.is_empty(),
        "PATH in env_vars alone must not change detect_modules command lookup"
    );
    Ok(())
}

#[tokio::test]
async fn detect_empty_env_value() {
    let defs = resolve_modules(&[ModuleDef {
        name: "empty_env".to_owned(),
        when: ModuleWhen {
            files: vec![],
            env: vec!["EMPTY_VAR".to_owned()],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("EMPTY_VAR".to_owned(), String::new())];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;
    let m = results.iter().find(|r| r.name == "empty_env");
    assert!(
        m.is_some(),
        "empty env var value should still trigger detection"
    );
    assert_eq!(m.map(|m| m.value.as_str()), Some(""));
}

#[tokio::test]
async fn detect_empty_file_filtered() -> Result<(), Box<dyn std::error::Error>> {
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
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Fast).await;
    assert!(
        results.iter().all(|r| r.name != "empty_file"),
        "empty file content must not produce a detection"
    );
    Ok(())
}

#[tokio::test]
async fn detect_file_traversal_rejected() -> Result<(), Box<dyn std::error::Error>> {
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
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let results = detect_modules(&defs, &sub, &[], None, ModuleSpeed::Fast).await;
    assert!(
        results.iter().all(|r| r.name != "traversal"),
        "file source with path traversal ('..') must be rejected"
    );
    Ok(())
}

#[tokio::test]
async fn detect_format_no_recursive_expansion() {
    let defs = resolve_modules(&[ModuleDef {
        name: "format_inject".to_owned(),
        when: ModuleWhen::default(),
        source: vec![SourceDef {
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let env_vars = vec![("INJECT_VAR".to_owned(), "{value}".to_owned())];
    let results =
        detect_modules(&defs, Path::new("/tmp"), &env_vars, None, ModuleSpeed::Fast).await;
    let m = results.iter().find(|r| r.name == "format_inject");
    assert_eq!(
        m.map(|m| m.value.as_str()),
        Some("prefix-{value}-suffix"),
        "{{value}} in raw value must not be recursively expanded"
    );
}

#[tokio::test]
async fn detect_command_no_shell_injection() -> Result<(), Box<dyn std::error::Error>> {
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
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let _results = detect_modules(&defs, dir.path(), &[], None, ModuleSpeed::Slow).await;
    assert!(
        !sentinel.exists(),
        "shell metacharacters in command args must not be interpreted"
    );
    Ok(())
}

#[tokio::test]
async fn detect_uses_declared_command_source_order() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("marker"), "")?;

    let defs = resolve_modules(&[ModuleDef {
        name: "runtime".to_owned(),
        when: ModuleWhen {
            files: vec!["marker".to_owned()],
            env: vec![],
        },
        source: vec![
            SourceDef {
                name: "value".to_owned(),
                env: None,
                file: None,
                command: Some(vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "sleep 0.05; echo slow".to_owned(),
                ]),
                regex: None,
            },
            SourceDef {
                name: "value".to_owned(),
                env: None,
                file: None,
                command: Some(vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "echo fast".to_owned(),
                ]),
                regex: None,
            },
        ],
        format: "{value}".to_owned(),
        icon: None,
        style: StyleConfig::default(),
        connector: None,
        arbitration: None,
        slot: ModuleSlot::default(),
    }]);
    let module = defs.iter().find(|resolved| resolved.name == "runtime");
    let Some(module) = module else {
        return Err("runtime module missing".into());
    };

    let facts = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let detected = facts.detect_module(module).await;

    assert_eq!(
        detected.as_ref().map(|info| info.value.as_str()),
        Some("slow"),
        "fallback sources must be resolved in declaration order"
    );
    Ok(())
}

#[tokio::test]
async fn detect_resolves_fast_file_source() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("marker"), "")?;
    std::fs::write(dir.path().join(".runtime-version"), "2.3.4\n")?;

    let defs = resolve_modules(&[ModuleDef {
        name: "runtime".to_owned(),
        when: ModuleWhen {
            files: vec!["marker".to_owned()],
            env: vec![],
        },
        source: vec![SourceDef {
            name: "value".to_owned(),
            env: None,
            file: Some(".runtime-version".to_owned()),
            command: None,
            regex: None,
        }],
        format: "v{value}".to_owned(),
        icon: None,
        style: StyleConfig::default(),
        connector: None,
        arbitration: None,
        slot: ModuleSlot::default(),
    }]);
    let module = defs.iter().find(|resolved| resolved.name == "runtime");
    let Some(module) = module else {
        return Err("runtime module missing".into());
    };

    let facts = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let detected = facts.detect_module(module).await;

    assert_eq!(
        detected.as_ref().map(|info| info.value.as_str()),
        Some("v2.3.4")
    );
    Ok(())
}

#[tokio::test]
async fn detect_concurrent_no_corruption() -> Result<(), Box<dyn std::error::Error>> {
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
            name: "value".to_owned(),
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
        slot: ModuleSlot::default(),
    }]);

    let defs = std::sync::Arc::new(defs);
    let dir_path = dir.path().to_path_buf();
    let mut handles = Vec::new();

    for _ in 0..8 {
        let defs = std::sync::Arc::clone(&defs);
        let path = dir_path.clone();
        handles.push(tokio::task::spawn(async move {
            detect_modules(&defs, &path, &[], None, ModuleSpeed::Fast).await
        }));
    }

    for handle in handles {
        let results = handle.await?;
        let m = results.iter().find(|r| r.name == "concurrent");
        assert!(m.is_some(), "each task must detect the module");
        assert_eq!(
            m.map(|m| m.value.as_str()),
            Some("v1.0.0"),
            "value must be consistent across tasks"
        );
    }
    Ok(())
}

// -- compute_dep_hash -----------------------------------------------------

#[test]
fn dep_hash_empty_inputs() {
    let facts = RequestFacts::collect(PathBuf::from("/tmp"), vec![]);
    let inputs = ModuleDependencyInputs::default();
    let h1 = inputs.compute_dep_hash(&facts);
    let h2 = inputs.compute_dep_hash(&facts);
    assert_eq!(h1, h2);
}

#[test]
fn dep_hash_same_env() {
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
fn dep_hash_different_env() {
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
fn dep_hash_env_present_absent() {
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
fn dep_hash_file_existence_changes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut inputs = ModuleDependencyInputs::default();
    inputs.trigger_files.push("marker".to_owned());

    let facts_no_file = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let h_without = inputs.compute_dep_hash(&facts_no_file);

    std::fs::write(dir.path().join("marker"), "")?;
    let facts_with_file = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let h_with = inputs.compute_dep_hash(&facts_with_file);

    assert_ne!(h_without, h_with);
    Ok(())
}

#[test]
fn dep_hash_source_file_content_changes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut inputs = ModuleDependencyInputs::default();
    inputs.source_files.push(".runtime-version".to_owned());

    std::fs::write(dir.path().join(".runtime-version"), "1.0.0\n")?;
    let facts_v1 = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let h_v1 = inputs.compute_dep_hash(&facts_v1);

    std::fs::write(dir.path().join(".runtime-version"), "2.0.0\n")?;
    let facts_v2 = RequestFacts::collect(dir.path().to_path_buf(), vec![]);
    let h_v2 = inputs.compute_dep_hash(&facts_v2);

    assert_ne!(
        h_v1, h_v2,
        "source file content must participate in slow cache keys"
    );
    Ok(())
}

#[test]
fn dep_hash_command_path_changes() {
    let inputs = ModuleDependencyInputs {
        uses_command_path: true,
        ..ModuleDependencyInputs::default()
    };

    let facts_a = RequestFacts::collect(PathBuf::from("/tmp"), vec![])
        .with_command_path_env(Some("/bin:/usr/bin".to_owned()));
    let facts_b = RequestFacts::collect(PathBuf::from("/tmp"), vec![])
        .with_command_path_env(Some("/opt/tools:/bin".to_owned()));

    assert_ne!(
        inputs.compute_dep_hash(&facts_a),
        inputs.compute_dep_hash(&facts_b),
        "forwarded PATH must participate when command sources are configured"
    );
}

#[test]
fn dep_hash_insertion_order_irrelevant() {
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
