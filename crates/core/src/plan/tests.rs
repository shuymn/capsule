use std::{ffi::OsString, fmt::Write, os::unix::ffi::OsStringExt};

use capsule_protocol::session::Snapshot;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    acquire::Runner,
    limits::{
        MAX_CANDIDATES_PER_VALUE, MAX_COMMAND_ARGS, MAX_CONFIG_BYTES, MAX_FORMAT_BYTES,
        MAX_FORMAT_DEPTH, MAX_MODULES, MAX_OUTPUT_BYTES, MAX_TOTAL_CANDIDATES, MAX_TOTAL_VALUES,
        MAX_VALUES_PER_MODULE, MAX_WHEN_ITEMS,
    },
    render::style::Color,
    test_utils::{pick, snapshot},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn one_module(format: &str, values: &str) -> Result<ConfigPlan, ConfigError> {
    ConfigPlan::parse(&format!(
        "schema_version = 2\n[[module]]\nname = 'test'\nformat = '{format}'\n[module.values]\n{values}\n"
    ))
}

/// `count` copies of a TOML `item`, comma-separated for an array body.
fn toml_list(item: &str, count: usize) -> String {
    vec![item; count].join(",")
}

#[test]
fn display_defaults_and_partial_styles_are_preserved() -> TestResult {
    let plan = ConfigPlan::parse(
        r#"
schema_version = 2
[character.success_style]
fg = "magenta"
[character.error_style]
bold = false
[character.vicmd]
glyph = "C"
[character.vicmd.style]
dimmed = true
[directory.style]
fg = "blue"
[git.state_style]
fg = "red"
[cmd_duration.style]
dimmed = true
[connectors.style]
fg = "bright_black"
[color_map]
blue = 94
[[module]]
name = "default-style"
[module.values]
value = [{ env = "VALUE" }]
[[module]]
name = "override-style"
style = { fg = "blue", bold = false }
[module.values]
value = [{ env = "VALUE" }]
"#,
    )?;
    assert_eq!(plan.view.character.glyph, "❯");
    assert_eq!(
        plan.modules[0].appearance.style,
        Style::new().fg(Color::BrightBlack).bold()
    );
    assert_eq!(
        plan.modules[1].appearance.style,
        Style::new().fg(Color::Blue)
    );
    assert_eq!(plan.view.character.vicmd.glyph, "C");
    assert_eq!(
        plan.view.character.success_prompt_style(),
        Style::new().fg(Color::Magenta).bold()
    );
    assert_eq!(
        plan.view.character.error_prompt_style(),
        Style::new().fg(Color::Red)
    );
    assert_eq!(
        plan.view.directory.prompt_style(),
        Style::new().fg(Color::Blue).bold()
    );
    assert_eq!(
        plan.view.git.state_prompt_style(),
        Style::new().fg(Color::Red).bold()
    );
    assert_eq!(
        plan.view.cmd_duration.prompt_style(),
        Style::new().fg(Color::Yellow).bold().dimmed()
    );
    assert!(plan.view.time.disabled);
    assert!(plan.view.time.show_seconds());
    assert_eq!(plan.view.cmd_duration.threshold_ms, 2000);
    assert_eq!(plan.view.git.connector, "on");
    assert_eq!(plan.view.color_map.blue, 94);
    assert_eq!(plan.modules.len(), 2);
    assert!(ConfigPlan::parse("schema_version = 2")?.modules.is_empty());
    Ok(())
}

/// Covers: T03
#[test]
fn schema_and_unknown_fields_are_rejected_at_every_boundary() {
    for source in [
        "",
        "schema_version = 1",
        "schema_version = 3",
        "schema_version = 2\nextra = 1",
        "schema_version = 2\n[timeout]\nslow_ms = 5",
        "schema_version = 2\n[cache]\nslow = 'off'",
        "schema_version = 2\n[character]\nextra = true",
        "schema_version = 2\n[character.vicmd]\nextra = true",
        "schema_version = 2\n[character.success_style]\nextra = true",
        "schema_version = 2\n[directory]\nextra = true",
        "schema_version = 2\n[git]\nextra = true",
        "schema_version = 2\n[time]\nextra = true",
        "schema_version = 2\n[cmd_duration]\nextra = true",
        "schema_version = 2\n[connectors]\nextra = true",
        "schema_version = 2\n[color_map]\nextra = 31",
        "schema_version = 2\n[[module]]\nname = 'x'\nsource = []\nvalues = {}",
        "schema_version = 2\n[[module]]\nname = 'x'\nvalues = {}\nextra = 1",
        "schema_version = 2\n[[module]]\nname = 'x'\nvalues = {}\nwhen = { extra = [] }",
        "schema_version = 2\n[[module]]\nname = 'x'\nvalues = {}\narbitration = { group = 'x', priority = 1, extra = 1 }",
    ] {
        assert!(ConfigPlan::parse(source).is_err(), "accepted {source:?}");
    }
    assert!(one_module("{value}", "value = [{ env = 'X', extra = 1 }]").is_err());
}

/// TOML strings that both source and condition paths must reject: anything
/// but a relative path of normal components without NUL.
const INVALID_PATHS: [&str; 7] = [
    "''",
    "'/absolute'",
    "'../parent'",
    "'./local'",
    "'nested/./local'",
    "'a/../b'",
    "\"a\\u0000b\"",
];

/// Covers: T03, T05
#[test]
fn source_and_regex_validation_happens_before_acquisition() {
    for candidate_text in [
        "[]",
        "[{}]",
        "[{ env = 'X', file = 'x' }]",
        "[{ env = '' }]",
        "[{ env = 'A=B' }]",
        "[{ command = [] }]",
        "[{ command = [''] }]",
        "[{ env = 'X', regex = '(' }]",
        "[{ env = 'X', regex = 'no-capture' }]",
        "[{ env = 'X', regex = '(a{1000000})' }]",
    ] {
        assert!(
            one_module("{value}", &format!("value = {candidate_text}")).is_err(),
            "accepted {candidate_text}"
        );
    }
    assert!(one_module("{value}", "value = [{ env = \"A\\u0000B\" }]").is_err());
    assert!(one_module("{value}", "value = [{ command = [\"x\", \"\\u0000\"] }]").is_err());
    let command = |count| format!("value = [{{ command = [{}] }}]", toml_list("'x'", count));
    assert!(one_module("{value}", &command(MAX_COMMAND_ARGS)).is_ok());
    assert!(one_module("{value}", &command(MAX_COMMAND_ARGS + 1)).is_err());
    for path in INVALID_PATHS {
        assert!(
            one_module("{value}", &format!("value = [{{ file = {path} }}]")).is_err(),
            "accepted file {path}"
        );
    }
}

/// Covers: T05
#[test]
fn conditions_are_relative_paths_and_bounded() {
    let when = |table: &str| {
        ConfigPlan::parse(&format!(
            "schema_version = 2\n[[module]]\nname = 'x'\nformat = 'x'\nvalues = {{}}\nwhen = {{ {table} }}"
        ))
    };
    assert!(when("files = ['Cargo.toml', 'nested/marker']").is_ok());
    for path in INVALID_PATHS {
        assert!(
            when(&format!("files = [{path}]")).is_err(),
            "accepted condition {path}"
        );
    }
    for key in ["files", "env"] {
        let items = |count| format!("{key} = [{}]", toml_list("'x'", count));
        assert!(when(&items(MAX_WHEN_ITEMS)).is_ok());
        assert!(when(&items(MAX_WHEN_ITEMS + 1)).is_err(), "accepted {key}");
    }
}

#[test]
fn declaration_order_and_candidate_order_are_retained() -> TestResult {
    let plan = ConfigPlan::parse(
        r#"
schema_version = 2
[[module]]
name = "z-last-alphabetically"
arbitration = { group = "runtime", priority = 20 }
[module.values]
value = [{ env = "VERSION" }, { file = "nested/version" }, { command = ["tool", ""] }]
[[module]]
name = "a-first-alphabetically"
arbitration = { group = "runtime", priority = 10 }
slot = "line2"
[module.values]
value = [{ env = "OTHER" }]
"#,
    )?;
    assert_eq!(plan.modules[0].name, "z-last-alphabetically");
    assert_eq!(plan.modules[1].name, "a-first-alphabetically");
    assert_eq!(plan.modules[1].slot, ModuleSlot::Line2);
    let candidates = &plan.modules[0].values[0].candidates;
    assert_eq!(candidates[0].source, Source::Env("VERSION".to_owned()));
    assert_eq!(candidates[1].source, Source::File("nested/version".into()));
    assert_eq!(
        candidates[2].source,
        Source::Command(vec!["tool".to_owned(), String::new()])
    );
    Ok(())
}

/// Covers: T03
#[test]
fn duplicate_names_and_undefined_or_unclosed_formats_are_rejected() -> TestResult {
    let source = "schema_version=2\n[[module]]\nname='x'\nvalues={value=[{env='A'}]}\n[[module]]\nname='x'\nvalues={value=[{env='B'}]}";
    assert!(ConfigPlan::parse(source).is_err());
    assert!(one_module("{value}", "value=[{env='A'}]\nvalue=[{env='B'}]").is_err());
    let named = |name: &str| {
        ConfigPlan::parse(&format!(
            "schema_version=2\n[[module]]\nname={name}\nformat='x'\nvalues={{}}"
        ))
    };
    assert!(named("'a b'").is_ok());
    for name in ["''", "' x'", "\"x\\u0007\"", "'a{b'"] {
        assert!(named(name).is_err(), "accepted name {name}");
    }
    for format in ["{missing}", "{", "{value", "[optional", "[{value}", "{}"] {
        assert!(one_module(format, "value=[{env='A'}]").is_err());
    }
    let plan = one_module(
        "{{literal} [[literal] {value}[ / {region}[ ({profile})]]",
        "value=[{env='A'}]\nregion=[{env='R'}]\nprofile=[{env='P'}]",
    )?;
    assert_eq!(
        plan.modules[0].format,
        Template(vec![
            FormatPart::Literal("{literal} [literal] ".to_owned()),
            FormatPart::Value(2),
            FormatPart::Optional(vec![
                FormatPart::Literal(" / ".to_owned()),
                FormatPart::Value(1),
                FormatPart::Optional(vec![
                    FormatPart::Literal(" (".to_owned()),
                    FormatPart::Value(0),
                    FormatPart::Literal(")".to_owned()),
                ]),
            ]),
        ])
    );
    Ok(())
}

#[test]
fn bracket_escapes_are_literal_inside_and_at_the_start_of_sections() -> TestResult {
    let plan = one_module("[{[}{value}{]}]{]}", "value=[{env='A'}]")?;
    assert_eq!(
        plan.modules[0].format,
        Template(vec![
            FormatPart::Optional(vec![
                FormatPart::Literal("[".to_owned()),
                FormatPart::Value(0),
                FormatPart::Literal("]".to_owned()),
            ]),
            FormatPart::Literal("]".to_owned()),
        ])
    );
    Ok(())
}

/// Covers: T03, T07
#[test]
fn configuration_bytes_and_format_depth_are_bounded() -> TestResult {
    let mut source = "schema_version=2\n#".to_owned();
    source.push_str(&"x".repeat(MAX_CONFIG_BYTES - source.len()));
    ConfigPlan::parse(&source)?;
    source.push('x');
    assert!(ConfigPlan::parse(&source).is_err());
    one_module(&"x".repeat(MAX_FORMAT_BYTES), "")?;
    assert!(one_module(&"x".repeat(MAX_FORMAT_BYTES + 1), "").is_err());
    let nested = |depth| {
        format!(
            "{}{value}{}",
            "[x ".repeat(depth),
            "]".repeat(depth),
            value = "{value}"
        )
    };
    one_module(&nested(MAX_FORMAT_DEPTH), "value=[{env='X'}]")?;
    assert!(one_module(&nested(MAX_FORMAT_DEPTH + 1), "value=[{env='X'}]").is_err());
    Ok(())
}

fn sized_plan(modules: usize, values: usize, candidates: usize) -> Result<String, std::fmt::Error> {
    let mut source = "schema_version=2\n".to_owned();
    for module in 0..modules {
        writeln!(
            source,
            "[[module]]\nname='m{module}'\nformat='literal'\n[module.values]"
        )?;
        for value in 0..values {
            writeln!(source, "v{value}=[{}]", toml_list("{env='X'}", candidates))?;
        }
    }
    Ok(source)
}

/// Covers: T03, T07
#[test]
fn module_value_and_candidate_counts_are_bounded() -> TestResult {
    ConfigPlan::parse(&sized_plan(MAX_MODULES, 1, 1)?)?;
    assert!(ConfigPlan::parse(&sized_plan(MAX_MODULES + 1, 1, 1)?).is_err());
    ConfigPlan::parse(&sized_plan(
        1,
        MAX_VALUES_PER_MODULE,
        MAX_CANDIDATES_PER_VALUE,
    )?)?;
    assert!(ConfigPlan::parse(&sized_plan(1, MAX_VALUES_PER_MODULE + 1, 1)?).is_err());
    assert!(ConfigPlan::parse(&sized_plan(1, 1, MAX_CANDIDATES_PER_VALUE + 1)?).is_err());
    ConfigPlan::parse(&sized_plan(16, 16, 1)?)?;
    assert!(ConfigPlan::parse(&sized_plan(17, 16, 1)?).is_err());
    ConfigPlan::parse(&sized_plan(8, 16, 8)?)?;
    assert!(ConfigPlan::parse(&sized_plan(9, 16, 8)?).is_err());
    Ok(())
}

/// Covers: T02
#[tokio::test]
async fn ready_empty_env_stops_fallback_but_unset_env_does_not() -> TestResult {
    let dir = tempfile::tempdir()?;
    let plan = one_module(
        "{value}",
        "value=[{env='VALUE'}, {command=['sh','-c','printf ran > fallback']}]",
    )?;
    let value_plan = &plan.modules[0].values[0];
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    let input = snapshot(dir.path(), &[("VALUE", "")])?;
    assert_eq!(
        acquire_value(value_plan, &input, &runner, &cancel).await?,
        Some(String::new())
    );
    assert!(!dir.path().join("fallback").exists());
    let input = snapshot(dir.path(), &[])?;
    assert_eq!(
        acquire_value(value_plan, &input, &runner, &cancel).await?,
        Some(String::new())
    );
    assert_eq!(std::fs::read_to_string(dir.path().join("fallback"))?, "ran");
    Ok(())
}

#[tokio::test]
async fn empty_file_and_command_are_ready_and_file_text_may_contain_slashes() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("value"), "")?;
    let plan = one_module("{value}", "value=[{file='value'},{env='FALLBACK'}]")?;
    let input = snapshot(dir.path(), &[("FALLBACK", "later")])?;
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    assert_eq!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
        Some(String::new())
    );
    std::fs::write(dir.path().join("value"), "  custom/team/channel\n")?;
    assert_eq!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
        Some("custom/team/channel".to_owned())
    );
    let plan = one_module(
        "{value}",
        "value=[{command=['sh','-c',':']},{env='FALLBACK'}]",
    )?;
    assert_eq!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
        Some(String::new())
    );
    Ok(())
}

/// Covers: T02
#[tokio::test]
async fn missing_failed_and_timed_out_candidates_fall_back_in_order() -> TestResult {
    let dir = tempfile::tempdir()?;
    let plan = one_module(
        "{value}",
        "value=[{file='missing'},{command=['sh','-c','exit 9']},{command=['sleep','2']},{env='GOOD'},{command=['sh','-c','printf ran > forbidden']}]",
    )?;
    let input = snapshot(dir.path(), &[("GOOD", "ready")])?;
    assert_eq!(
        acquire_value(
            &plan.modules[0].values[0],
            &input,
            &Runner::default(),
            &CancellationToken::new()
        )
        .await?,
        Some("ready".to_owned())
    );
    assert!(!dir.path().join("forbidden").exists());
    Ok(())
}

#[tokio::test]
async fn missing_and_failed_terminal_states_remain_distinct() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = snapshot(dir.path(), &[])?;
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    let missing = one_module("{value}", "value=[{env='MISSING'},{file='missing'}]")?;
    assert!(
        acquire_value(&missing.modules[0].values[0], &input, &runner, &cancel)
            .await?
            .is_none()
    );
    let failed = one_module(
        "{value}",
        "value=[{command=['sh','-c','exit 9']},{env='MISSING'}]",
    )?;
    assert!(
        acquire_value(&failed.modules[0].values[0], &input, &runner, &cancel)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn regex_extraction_distinguishes_empty_capture_from_missing_capture() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = snapshot(dir.path(), &[("VALUE", ""), ("FALLBACK", "later")])?;
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    let empty = one_module(
        "{value}",
        "value=[{env='VALUE',regex='^()$'},{env='FALLBACK'}]",
    )?;
    assert_eq!(
        acquire_value(&empty.modules[0].values[0], &input, &runner, &cancel).await?,
        Some(String::new())
    );
    for regex in ["^(x)$", "^(x)?$"] {
        let plan = one_module(
            "{value}",
            &format!("value=[{{env='VALUE',regex='{regex}'}},{{env='FALLBACK'}}]"),
        )?;
        assert_eq!(
            acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
            Some("later".to_owned())
        );
    }
    Ok(())
}

#[tokio::test]
async fn conditions_preserve_empty_environment_and_or_and_semantics() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("marker"), "")?;
    let when = ModuleWhen {
        files: vec!["missing".into(), "marker".into()],
        env: vec!["MISSING".to_owned(), "EMPTY".to_owned()],
    };
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    assert!(
        acquire_condition(
            &when,
            &snapshot(dir.path(), &[("EMPTY", "")])?,
            &runner,
            &cancel
        )
        .await?
    );
    assert!(!acquire_condition(&when, &snapshot(dir.path(), &[])?, &runner, &cancel).await?);
    std::fs::remove_file(dir.path().join("marker"))?;
    std::fs::create_dir(dir.path().join("marker"))?;
    assert!(
        !acquire_condition(
            &when,
            &snapshot(dir.path(), &[("EMPTY", "")])?,
            &runner,
            &cancel
        )
        .await?
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_does_not_fall_back_to_a_ready_value() -> TestResult {
    let dir = tempfile::tempdir()?;
    let plan = one_module("{value}", "value=[{env='VALUE'}]")?;
    let input = snapshot(dir.path(), &[("VALUE", "ready")])?;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let runner = Runner::default();
    assert!(matches!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await,
        Err(AcquireError::Cancelled)
    ));
    assert!(matches!(
        acquire_condition(&ModuleWhen::default(), &input, &runner, &cancel).await,
        Err(AcquireError::Cancelled)
    ));
    Ok(())
}

/// Covers: T07
#[tokio::test]
async fn environment_size_limit_applies_before_copying_and_allows_fallback() -> TestResult {
    let dir = tempfile::tempdir()?;
    let plan = one_module("{value}", "value=[{env='LARGE'},{env='FALLBACK'}]")?;
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    for (large, expected) in [
        (
            "x".repeat(MAX_OUTPUT_BYTES).into(),
            "x".repeat(MAX_OUTPUT_BYTES),
        ),
        ("x".repeat(MAX_OUTPUT_BYTES + 1).into(), "small".to_owned()),
        (
            OsString::from_vec(vec![0xff; MAX_OUTPUT_BYTES]),
            "small".to_owned(),
        ),
    ] {
        let env = vec![("FALLBACK".into(), "small".into()), ("LARGE".into(), large)];
        let input = Snapshot::new(dir.path().to_owned(), env)?;
        assert_eq!(
            acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
            Some(expected)
        );
    }
    Ok(())
}

mod template_properties {
    use proptest::prelude::*;

    use super::*;
    use crate::test_utils::{format_parts, hostile_text};

    const NAMES: [&str; 2] = ["a", "b"];

    fn values() -> Vec<ValuePlan> {
        NAMES
            .iter()
            .map(|name| ValuePlan {
                name: (*name).to_owned(),
                candidates: Vec::new(),
            })
            .collect()
    }

    /// A format tree the syntax can express: a section does not start with a
    /// nested section, because `[[` is a literal `[` (docs/extending.md).
    fn tree(depth: u32) -> impl Strategy<Value = Vec<FormatPart>> {
        format_parts(NAMES.len(), depth).prop_map(|parts| normalize(parts, false))
    }

    fn normalize(parts: Vec<FormatPart>, nested: bool) -> Vec<FormatPart> {
        let mut parts: Vec<_> = parts
            .into_iter()
            .map(|part| match part {
                FormatPart::Optional(parts) => FormatPart::Optional(normalize(parts, true)),
                part => part,
            })
            .collect();
        let first = parts
            .iter()
            .find(|part| !matches!(part, FormatPart::Literal(text) if text.is_empty()));
        if nested && matches!(first, Some(FormatPart::Optional(_))) {
            parts.insert(0, FormatPart::Literal(" ".into()));
        }
        parts
    }

    /// Prints brackets as `[[` and `]` outside sections, and as `{[}` and
    /// `{]}` inside them, so both escape forms are exercised.
    fn print(parts: &[FormatPart], nested: bool, source: &mut String) {
        for part in parts {
            match part {
                FormatPart::Literal(text) => {
                    let text = text.replace('{', "{{");
                    source.push_str(&if nested {
                        text.replace('[', "{[}").replace(']', "{]}")
                    } else {
                        text.replace('[', "[[")
                    });
                }
                FormatPart::Value(index) => {
                    source.push('{');
                    source.push_str(NAMES[*index]);
                    source.push('}');
                }
                FormatPart::Optional(parts) => {
                    source.push('[');
                    print(parts, true, source);
                    source.push(']');
                }
            }
        }
    }

    proptest! {
        #[test]
        fn compiling_arbitrary_source_never_panics(source in hostile_text(128)) {
            let _ = template::compile(&source, &values(), "test");
        }

        #[test]
        fn printed_trees_compile_back_to_themselves(parts in tree(3)) {
            let mut source = String::new();
            print(&parts, false, &mut source);
            let compiled = template::compile(&source, &values(), "test");
            let Ok(Template(compiled)) = compiled else {
                return Err(TestCaseError::fail(format!("{source:?}: {compiled:?}")));
            };
            // Compare printed forms, so how literals are split is not part of
            // the property.
            let mut reprinted = String::new();
            print(&compiled, false, &mut reprinted);
            prop_assert_eq!(reprinted, source);
        }
    }
}

/// Panics unless a configuration `source` is rejected or compiles within every
/// count and depth limit in threat-model.md §6; the regex builder bounds
/// regex size and nesting.
fn assert_accepted_within_limits(source: &str) {
    fn depth(parts: &[FormatPart]) -> usize {
        parts
            .iter()
            .map(|part| match part {
                FormatPart::Optional(parts) => depth(parts) + 1,
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }
    let Ok(plan) = ConfigPlan::parse(source) else {
        return;
    };
    let values = || plan.modules.iter().flat_map(|module| &module.values);
    let candidates = || values().flat_map(|value| &value.candidates);
    assert!(plan.modules.len() <= MAX_MODULES);
    assert!(values().count() <= MAX_TOTAL_VALUES);
    assert!(candidates().count() <= MAX_TOTAL_CANDIDATES);
    for module in &plan.modules {
        assert!(module.values.len() <= MAX_VALUES_PER_MODULE);
        assert!(
            module.when.files.len() <= MAX_WHEN_ITEMS && module.when.env.len() <= MAX_WHEN_ITEMS
        );
        assert!(depth(&module.format.0) <= MAX_FORMAT_DEPTH);
    }
    for value in values() {
        assert!((1..=MAX_CANDIDATES_PER_VALUE).contains(&value.candidates.len()));
    }
    for candidate in candidates() {
        assert!(
            !matches!(&candidate.source, Source::Command(args) if args.len() > MAX_COMMAND_ARGS)
        );
    }
}

/// Covers: T07
#[test]
fn arbitrary_configuration_text_respects_the_plan_limits() {
    bolero::check!().for_each(|input: &[u8]| {
        assert_accepted_within_limits(&String::from_utf8_lossy(input));
    });
}

/// Modules whose fields pick from fixed fragments, so generated
/// configurations mostly compile, and each limit is sometimes exceeded.
#[derive(Debug, bolero::TypeGenerator)]
struct GeneratedConfig {
    #[generator(bolero::produce::<Vec<GeneratedModule>>().with().len(0..=4usize))]
    modules: Vec<GeneratedModule>,
    /// A [`sized_plan`] prefix that can pass `MAX_MODULES` and the plan-wide
    /// totals within `MAX_CONFIG_BYTES`.
    #[generator(0..=MAX_MODULES + 6)]
    padding_modules: usize,
    #[generator(0..=4usize)]
    padding_values: usize,
    #[generator(1..=MAX_CANDIDATES_PER_VALUE)]
    padding_candidates: usize,
}

#[derive(Debug, bolero::TypeGenerator)]
struct GeneratedModule {
    #[generator(0..=MAX_WHEN_ITEMS + 1)]
    files: usize,
    #[generator(0..=MAX_WHEN_ITEMS + 1)]
    env: usize,
    #[generator(bolero::produce::<Vec<u8>>().with().len(0..=16usize))]
    format: Vec<u8>,
    #[generator(
        bolero::produce::<Vec<Vec<u8>>>()
            .with()
            .len(0..=MAX_VALUES_PER_MODULE + 1)
            .values(bolero::produce::<Vec<u8>>().with().len(1..=MAX_CANDIDATES_PER_VALUE + 1))
    )]
    values: Vec<Vec<u8>>,
    line2: bool,
    priority: Option<u8>,
}

fn generated_source(config: &GeneratedConfig) -> Result<String, std::fmt::Error> {
    let mut source = sized_plan(
        config.padding_modules,
        config.padding_values,
        config.padding_candidates,
    )?;
    let deepest = format!(
        "{}{}",
        "[x".repeat(MAX_FORMAT_DEPTH + 1),
        "]".repeat(MAX_FORMAT_DEPTH + 1)
    );
    let last_value = format!("{{v{MAX_VALUES_PER_MODULE}}}");
    let format_tokens = [
        "{v0}",
        "{v1}",
        &last_value,
        "[x",
        "]",
        "{{",
        "[[",
        "x",
        &deepest,
    ];
    let many_args = format!("{{command=[{}]}}", toml_list("'a'", MAX_COMMAND_ARGS + 1));
    let candidate_tokens = [
        "{env='A'}",
        "{file='f'}",
        "{command=['c', '-v'], regex='(\\d+)'}",
        "{env='B', regex='((((a+)+)+)+){1000}'}",
        "{env='A', file='f'}",
        &many_args,
    ];
    let names = |prefix: &str, count: usize| {
        (0..count)
            .map(|index| format!("'{prefix}{index}'"))
            .collect::<Vec<_>>()
            .join(",")
    };
    for (index, module) in config.modules.iter().enumerate() {
        writeln!(source, "[[module]]\nname='g{index}'")?;
        writeln!(
            source,
            "when.files=[{}]\nwhen.env=[{}]",
            names("f", module.files),
            names("E", module.env)
        )?;
        let format = module
            .format
            .iter()
            .map(|choice| pick(*choice, &format_tokens))
            .collect::<String>();
        writeln!(source, "format={format:?}")?;
        if module.line2 {
            source.push_str("slot='line2'\n");
        }
        if let Some(priority) = module.priority {
            writeln!(source, "arbitration={{group='g', priority={priority}}}")?;
        }
        source.push_str("[module.values]\n");
        for (value, choices) in module.values.iter().enumerate() {
            let choices = choices
                .iter()
                .map(|choice| pick(*choice, &candidate_tokens))
                .collect::<Vec<_>>();
            writeln!(source, "v{value}=[{}]", choices.join(","))?;
        }
    }
    Ok(source)
}

/// Covers: T07
#[test]
fn generated_configurations_respect_the_plan_limits() {
    bolero::check!()
        .with_type::<GeneratedConfig>()
        .for_each(|config| {
            if let Ok(source) = generated_source(config) {
                assert_accepted_within_limits(&source);
            }
        });
}
