use crate::{
    config::{Arbitration, ModuleDef, ModuleWhen, RegexPattern, SourceDef, StyleConfig},
    render::style::Color,
};

pub(super) const JS_RUNTIME_ARBITRATION_GROUP: &str = "node.js";
pub(super) const BUN_ARBITRATION_PRIORITY: u32 = 10;
pub(super) const NODE_ARBITRATION_PRIORITY: u32 = 20;

/// Returns the built-in toolchain definitions as [`ModuleDef`]s.
#[must_use]
pub(super) fn builtin_module_defs() -> Vec<ModuleDef> {
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
            style: StyleConfig::fg(Color::Red),
            connector: Some("via".to_owned()),
            arbitration: None,
        },
        ModuleDef {
            name: "bun".to_owned(),
            when: ModuleWhen {
                files: vec![
                    "bun.lock".to_owned(),
                    "bun.lockb".to_owned(),
                    "bunfig.toml".to_owned(),
                ],
                env: vec![],
            },
            source: vec![
                SourceDef {
                    env: None,
                    file: Some(".bun-version".to_owned()),
                    command: None,
                    regex: None,
                },
                SourceDef {
                    env: None,
                    file: None,
                    command: Some(vec!["bun".to_owned(), "--version".to_owned()]),
                    regex: Some(RegexPattern::new_unchecked(r"(\d[\d.]*)".to_owned())),
                },
            ],
            format: "v{value}".to_owned(),
            icon: Some("\u{e76f}".to_owned()),
            style: StyleConfig::fg(Color::Red),
            connector: Some("via".to_owned()),
            arbitration: Some(Arbitration {
                group: JS_RUNTIME_ARBITRATION_GROUP.to_owned(),
                priority: BUN_ARBITRATION_PRIORITY,
            }),
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
            style: StyleConfig::fg(Color::Green),
            connector: Some("via".to_owned()),
            arbitration: Some(Arbitration {
                group: JS_RUNTIME_ARBITRATION_GROUP.to_owned(),
                priority: NODE_ARBITRATION_PRIORITY,
            }),
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
            style: StyleConfig::fg(Color::Cyan),
            connector: Some("via".to_owned()),
            arbitration: None,
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
            style: StyleConfig::fg(Color::Yellow),
            connector: Some("via".to_owned()),
            arbitration: None,
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
            style: StyleConfig::fg(Color::Red),
            connector: Some("via".to_owned()),
            arbitration: None,
        },
    ]
}
