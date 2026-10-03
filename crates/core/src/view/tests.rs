use super::*;
use crate::{
    plan::ViewConfig,
    render::style::{Color, ColorMap},
    test_utils::contains_style_sequence,
};

fn input() -> ViewInput<'static> {
    ViewInput {
        directory: "project",
        read_only: false,
        git: None,
        modules: &[],
        cols: 100,
        last_exit_code: 0,
        duration_ms: None,
        keymap: "main",
        time: None,
    }
}

#[test]
fn required_pending_hides_module_but_optional_pending_omits_only_its_section() {
    let parts = vec![
        FormatPart::Literal("v".into()),
        FormatPart::Value(0),
        FormatPart::Optional(vec![
            FormatPart::Literal(" (".into()),
            FormatPart::Value(1),
            FormatPart::Literal(")".into()),
        ]),
    ];
    assert!(
        evaluate_format(
            &parts,
            &[Observation::Pending, Observation::Ready("x".into())]
        )
        .is_none()
    );
    assert_eq!(
        evaluate_format(
            &parts,
            &[Observation::Ready("1".into()), Observation::Pending]
        ),
        Some("v1".into())
    );
    assert_eq!(
        evaluate_format(
            &parts,
            &[Observation::Ready("1".into()), Observation::Missing]
        ),
        Some("v1".into())
    );
    assert_eq!(
        evaluate_format(
            &parts,
            &[
                Observation::Ready("1".into()),
                Observation::Ready(String::new())
            ]
        ),
        Some("v1 ()".into())
    );
}

#[test]
fn nested_optional_sections_have_independent_availability() {
    let parts = vec![FormatPart::Optional(vec![
        FormatPart::Value(0),
        FormatPart::Optional(vec![FormatPart::Literal("/".into()), FormatPart::Value(1)]),
    ])];
    assert_eq!(
        evaluate_format(
            &parts,
            &[Observation::Ready("outer".into()), Observation::Missing]
        ),
        Some("outer".into())
    );
    assert_eq!(
        evaluate_format(
            &parts,
            &[Observation::Pending, Observation::Ready("inner".into())]
        ),
        Some(String::new())
    );
}

#[test]
fn line_fitting_truncates_first_segment_then_drops_the_rightmost() {
    let segment = |text| StyledText::new(text, Style::new());
    let truncated = fit_line(&[segment("very/long/directory"), segment("main")], 10);
    assert_eq!(truncated.to_zsh(ColorMap::default()), "very… main");
    let dropped = fit_line(
        &[
            segment("dir"),
            segment("segment-aaa"),
            segment("segment-bbb"),
        ],
        20,
    );
    assert_eq!(dropped.to_zsh(ColorMap::default()), "dir segment-aaa");
    assert!(fit_line(&[segment("abc")], 0).is_empty());
}

#[test]
fn character_keymap_and_status_use_current_inputs_only() {
    let mut config = ViewConfig::default();
    let mut input = input();
    assert!(
        builtins::character(&config, &input)
            .to_zsh(config.color_map)
            .contains("❯")
    );
    input.last_exit_code = 1;
    assert!(contains_style_sequence(
        &builtins::character(&config, &input).to_zsh(config.color_map),
        &[1, 31]
    ));
    input.keymap = "vicmd";
    let output = builtins::character(&config, &input).to_zsh(config.color_map);
    assert!(output.contains("❮"));
    assert!(contains_style_sequence(&output, &[1, 31]));
    config.character.vicmd.style = Some(crate::plan::StyleConfig::fg(Color::Blue));
    assert!(
        builtins::character(&config, &input)
            .to_zsh(config.color_map)
            .contains("\x1b[34m")
    );
}

#[test]
fn literal_data_is_safe_with_prompt_substitution_on_and_off()
-> Result<(), Box<dyn std::error::Error>> {
    let data = "literal $HOME $(printf EXPANDED) `printf EXPANDED` \\ %B %{data%}";
    let serialized = StyledText::new(data, Style::new()).to_zsh(ColorMap::default());
    let output = std::process::Command::new("zsh")
        .args([
            "-f",
            "-c",
            r#"
rendered=$VIEW_PROMPT
setopt promptsubst
PROMPT='${rendered}'
print -P -r -- "$PROMPT"
unsetopt promptsubst
PROMPT=$rendered
print -P -r -- "$PROMPT"
"#,
        ])
        .env("VIEW_PROMPT", serialized)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!("{data}\n{data}\n")
    );
    Ok(())
}

fn observed(value: &str) -> ModuleObservation {
    ModuleObservation {
        condition: Observation::Ready(true),
        values: vec![Observation::Ready(value.into())],
    }
}

#[test]
fn arbitration_runs_once_across_all_sources_and_slots_in_declaration_order()
-> Result<(), Box<dyn std::error::Error>> {
    let mut plan = ConfigPlan::parse(
        r#"
schema_version = 2
[directory]
disabled = true
[character]
disabled = true

[[module]]
name = "before"
values = { value = [{ env = "BEFORE" }] }
[[module]]
name = "preferred"
values = { value = [{ command = ["unused"] }] }
arbitration = { group = "toolchain", priority = 1 }
[[module]]
name = "fallback"
values = { value = [{ env = "FALLBACK" }] }
arbitration = { group = "toolchain", priority = 2 }
[[module]]
name = "middle"
values = { value = [{ env = "MIDDLE" }] }
[[module]]
name = "tie"
slot = "line2"
values = { value = [{ file = "unused" }] }
arbitration = { group = "toolchain", priority = 1 }
[[module]]
name = "after"
values = { value = [{ env = "AFTER" }] }
"#,
    )?;
    // Keep this ordering contract independent of the separately tested defaults.
    for module in &mut plan.modules {
        module.appearance.style = Style::new();
    }
    let mut observations = vec![
        observed("before"),
        ModuleObservation::pending(&plan.modules[1]),
        observed("fallback"),
        observed("middle"),
        ModuleObservation::pending(&plan.modules[4]),
        observed("after"),
    ];
    let pending = render(
        &plan,
        &ViewInput {
            modules: &observations,
            ..input()
        },
    );
    assert_eq!(pending.left1, "before fallback middle after");
    assert_eq!(pending.left2, "");

    observations[4] = observed("tie");
    let file_ready = render(
        &plan,
        &ViewInput {
            modules: &observations,
            ..input()
        },
    );
    assert_eq!(file_ready.left1, "before middle after");
    assert_eq!(file_ready.left2, "tie");

    observations[1] = observed("preferred");
    let all_ready = render(
        &plan,
        &ViewInput {
            modules: &observations,
            ..input()
        },
    );
    assert_eq!(all_ready.left1, "before preferred middle after");
    assert_eq!(all_ready.left2, "");

    observations[1].condition = Observation::Ready(false);
    let hidden = render(
        &plan,
        &ViewInput {
            modules: &observations,
            ..input()
        },
    );
    assert_eq!(hidden.left1, "before middle after");
    assert_eq!(hidden.left2, "tie");
    Ok(())
}

#[test]
fn builtins_keep_two_lines_connectors_and_independent_styles() {
    use crate::git::{GitOperationState, GitState};

    let mut plan = ConfigPlan::default();
    plan.view.time.disabled = false;
    let status = GitStatus {
        head_oid: Some("abcdef012345".into()),
        modified: 1,
        state: Some(GitOperationState {
            state: GitState::Rebase,
            step: Some(2),
            total: Some(5),
        }),
        ..GitStatus::default()
    };
    let prompt = render(
        &plan,
        &ViewInput {
            git: Some(&status),
            read_only: true,
            duration_ms: Some(65_000),
            time: Some((14, 5, 9)),
            ..input()
        },
    );
    for text in [
        "project",
        "\u{f023}",
        "on",
        "\u{f418}",
        "HEAD ",
        "(abcdef0)",
        "(REBASING 2/5)",
        "[!]",
        "took",
        "1m5s",
    ] {
        assert!(
            prompt.left1.contains(text),
            "missing {text}: {}",
            prompt.left1
        );
    }
    assert!(contains_style_sequence(&prompt.left1, &[1, 36]));
    assert!(contains_style_sequence(&prompt.left1, &[1, 35]));
    assert!(contains_style_sequence(&prompt.left1, &[1, 32]));
    assert!(contains_style_sequence(&prompt.left1, &[1, 33]));
    assert!(contains_style_sequence(&prompt.left1, &[1, 31]));
    for text in ["at", "14:05:09", "❯"] {
        assert!(
            prompt.left2.contains(text),
            "missing {text}: {}",
            prompt.left2
        );
    }
}

#[test]
fn builtins_disabled_and_unavailable_values_stay_absent() {
    let mut plan = ConfigPlan::default();
    plan.view.directory.disabled = true;
    plan.view.character.disabled = true;
    let output = render(&plan, &input());
    assert_eq!(output, PromptLines::default());
    plan.view.time.disabled = false;
    assert_eq!(render(&plan, &input()), PromptLines::default());
    plan.view.cmd_duration.disabled = true;
    assert_eq!(
        render(
            &plan,
            &ViewInput {
                duration_ms: Some(90_000),
                ..input()
            }
        ),
        PromptLines::default()
    );
}

#[test]
fn width_and_keymap_redraw_reuse_the_same_observations() -> Result<(), Box<dyn std::error::Error>> {
    let plan = ConfigPlan::parse(
        r#"
schema_version = 2
[[module]]
name = "rust"
connector = "via"
values = { value = [{ command = ["never-run-by-view"] }] }
"#,
    )?;
    let observations = [observed("👩🏽‍💻-value")];
    let normal = render(
        &plan,
        &ViewInput {
            modules: &observations,
            ..input()
        },
    );
    let narrow = render(
        &plan,
        &ViewInput {
            modules: &observations,
            cols: 10,
            keymap: "vicmd",
            ..input()
        },
    );
    assert!(normal.left1.contains("👩🏽‍💻-value"));
    assert!(normal.left2.contains("❯"));
    assert!(narrow.left2.contains("❮"));
    assert!(matches!(&observations[0].values[0], Observation::Ready(value) if value == "👩🏽‍💻-value"));
    Ok(())
}

#[test]
fn repeated_references_cannot_amplify_bounded_values() {
    let value = "x".repeat(MAX_MODULE_TEXT_BYTES);
    let observed = [Observation::Ready(value)];
    assert!(evaluate_format(&[FormatPart::Value(0)], &observed).is_some());
    assert!(evaluate_format(&[FormatPart::Value(0), FormatPart::Value(0)], &observed).is_none());
}

#[test]
fn pathological_zero_width_data_still_fits_the_response_byte_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let mut plan = ConfigPlan::default();
    plan.view.character.glyph = "\\%".repeat(32_000);
    let combining = format!("a{}", "\u{301}".repeat(32_000));
    let output = render(
        &plan,
        &ViewInput {
            directory: &combining,
            cols: usize::from(u16::MAX),
            ..input()
        },
    );
    assert!(output.left1.contains('…'));
    assert!(
        !output.left1.contains('\u{301}'),
        "a single oversized grapheme is omitted intact"
    );
    let frame = capsule_protocol::session::response(u64::MAX, &output.left1, &output.left2, true)?;
    assert!(frame.len() <= capsule_protocol::session::MAX_RESPONSE + 1);
    Ok(())
}
