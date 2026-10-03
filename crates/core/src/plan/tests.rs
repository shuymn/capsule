use std::{ffi::OsString, fmt::Write, os::unix::ffi::OsStringExt};

use capsule_protocol::session::Snapshot;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    acquire::{MAX_OUTPUT_BYTES, Runner},
    render::style::Color,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn one_module(format: &str, values: &str) -> Result<ConfigPlan, ConfigError> {
    ConfigPlan::parse(&format!(
        "schema_version = 2\n[[module]]\nname = 'test'\nformat = '{format}'\n[module.values]\n{values}\n"
    ))
}

fn snapshot(cwd: &std::path::Path, env: &[(&str, &str)]) -> Snapshot {
    Snapshot {
        cwd: cwd.to_path_buf(),
        env: env
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .chain(std::env::var_os("PATH").map(|value| (OsString::from("PATH"), value)))
            .collect(),
    }
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

#[test]
fn source_and_regex_validation_happens_before_acquisition() {
    for candidate_text in [
        "[]",
        "[{}]",
        "[{ env = 'X', file = 'x' }]",
        "[{ env = '' }]",
        "[{ env = 'A=B' }]",
        "[{ file = '' }]",
        "[{ file = '/absolute' }]",
        "[{ file = '../parent' }]",
        "[{ file = './local' }]",
        "[{ file = 'nested/./local' }]",
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

#[test]
fn duplicate_names_and_undefined_or_unclosed_formats_are_rejected() -> TestResult {
    let source = "schema_version=2\n[[module]]\nname='x'\nvalues={value=[{env='A'}]}\n[[module]]\nname='x'\nvalues={value=[{env='B'}]}";
    assert!(ConfigPlan::parse(source).is_err());
    assert!(one_module("{value}", "value=[{env='A'}]\nvalue=[{env='B'}]").is_err());
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
            writeln!(
                source,
                "v{value}=[{}]",
                vec!["{env='X'}"; candidates].join(",")
            )?;
        }
    }
    Ok(source)
}

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
    let input = snapshot(dir.path(), &[("VALUE", "")]);
    assert_eq!(
        acquire_value(value_plan, &input, &runner, &cancel).await?,
        Some(String::new())
    );
    assert!(!dir.path().join("fallback").exists());
    let input = snapshot(dir.path(), &[]);
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
    let input = snapshot(dir.path(), &[("FALLBACK", "later")]);
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

#[tokio::test]
async fn missing_failed_and_timed_out_candidates_fall_back_in_order() -> TestResult {
    let dir = tempfile::tempdir()?;
    let plan = one_module(
        "{value}",
        "value=[{file='missing'},{command=['sh','-c','exit 9']},{command=['sleep','2']},{env='GOOD'},{command=['sh','-c','printf ran > forbidden']}]",
    )?;
    let input = snapshot(dir.path(), &[("GOOD", "ready")]);
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
    let input = snapshot(dir.path(), &[]);
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
    let input = snapshot(dir.path(), &[("VALUE", ""), ("FALLBACK", "later")]);
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
            &snapshot(dir.path(), &[("EMPTY", "")]),
            &runner,
            &cancel
        )
        .await?
    );
    assert!(!acquire_condition(&when, &snapshot(dir.path(), &[]), &runner, &cancel).await?);
    std::fs::remove_file(dir.path().join("marker"))?;
    assert!(
        !acquire_condition(
            &when,
            &snapshot(dir.path(), &[("EMPTY", "")]),
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
    let input = snapshot(dir.path(), &[("VALUE", "ready")]);
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

#[tokio::test]
async fn environment_size_limit_applies_before_copying_and_allows_fallback() -> TestResult {
    let dir = tempfile::tempdir()?;
    let plan = one_module("{value}", "value=[{env='LARGE'},{env='FALLBACK'}]")?;
    let mut input = snapshot(dir.path(), &[("FALLBACK", "small")]);
    let large_index = input.env.len();
    input
        .env
        .push(("LARGE".into(), "x".repeat(MAX_OUTPUT_BYTES).into()));
    let runner = Runner::default();
    let cancel = CancellationToken::new();
    assert_eq!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel)
            .await?
            .as_ref()
            .map(String::len),
        Some(MAX_OUTPUT_BYTES)
    );
    input.env[large_index].1.push("x");
    assert_eq!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
        Some("small".to_owned())
    );
    input.env[large_index].1 = OsString::from_vec(vec![0xff; MAX_OUTPUT_BYTES]);
    assert_eq!(
        acquire_value(&plan.modules[0].values[0], &input, &runner, &cancel).await?,
        Some("small".to_owned())
    );
    assert_eq!(
        input.env[large_index].1.as_encoded_bytes(),
        vec![0xff; MAX_OUTPUT_BYTES]
    );
    Ok(())
}
