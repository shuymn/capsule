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

/// Covers: T01
#[test]
fn literal_data_is_safe_with_prompt_substitution_on_and_off()
-> Result<(), Box<dyn std::error::Error>> {
    let data = "literal $HOME $(printf EXPANDED) `printf EXPANDED` \\$ %B %{data%} \\";
    let serialized = StyledText::new(data, Style::new()).to_zsh(ColorMap::default());
    // Direct text under PROMPT_SUBST models an option change after Capsule's
    // last refresh, such as a failing named precmd hook. The shell's line
    // separator follows a trailing backslash.
    let output = std::process::Command::new("zsh")
        .args([
            "-f",
            "-c",
            r#"
rendered=$VIEW_PROMPT$'\n'$VIEW_PROMPT
setopt promptsubst
print -P -r -- '${rendered}'
print -P -r -- "$rendered"
unsetopt promptsubst
print -P -r -- "$rendered"
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
        format!("{data}\n{data}\n").repeat(3)
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

/// Covers: T07
#[test]
fn repeated_references_cannot_amplify_bounded_values() {
    let value = "x".repeat(MAX_MODULE_TEXT_BYTES);
    let observed = [Observation::Ready(value)];
    assert!(evaluate_format(&[FormatPart::Value(0)], &observed).is_some());
    assert!(evaluate_format(&[FormatPart::Value(0), FormatPart::Value(0)], &observed).is_none());
}

/// Covers: T07
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

/// Covers: T07
#[test]
fn quoted_backslashes_on_both_lines_fit_the_response_byte_limit()
-> Result<(), Box<dyn std::error::Error>> {
    // A backslash has the largest prompt-quoting and wire-escaping expansion.
    let data = "\\".repeat(32_000);
    let mut plan = ConfigPlan::default();
    plan.view.character.glyph.clone_from(&data);
    let output = render(
        &plan,
        &ViewInput {
            directory: &data,
            cols: usize::from(u16::MAX),
            ..input()
        },
    );
    assert!(output.left1.contains('…') && output.left2.contains('…'));
    let frame = capsule_protocol::session::response(u64::MAX, &output.left1, &output.left2, true)?;
    assert!(frame.len() <= capsule_protocol::session::MAX_RESPONSE + 1);
    Ok(())
}

mod properties {
    use proptest::prelude::*;
    use unicode_width::UnicodeWidthStr as _;

    use super::*;
    use crate::{
        acquire::AcquireError,
        plan::{Arbitration, ModuleAppearance, ModuleWhen, Template, ValuePlan},
        test_utils::{bulky_text, displayed_prompt, hostile_text},
    };

    fn format(values: usize, depth: u32) -> BoxedStrategy<Vec<FormatPart>> {
        let leaf = if values == 0 {
            hostile_text(6).prop_map(FormatPart::Literal).boxed()
        } else {
            prop_oneof![
                hostile_text(6).prop_map(FormatPart::Literal),
                (0..values).prop_map(FormatPart::Value),
            ]
            .boxed()
        };
        let part = leaf.prop_recursive(depth, 12, 4, |inner| {
            prop::collection::vec(inner, 0..4).prop_map(FormatPart::Optional)
        });
        prop::collection::vec(part, 0..5).boxed()
    }

    fn module() -> impl Strategy<Value = ModulePlan> {
        (0..3usize)
            .prop_flat_map(|values| {
                (
                    Just(values),
                    format(values, 3),
                    prop::option::of((prop::sample::select(vec!["a", "b"]), 0..3u32)),
                    prop::bool::ANY,
                    prop::option::of(hostile_text(4)),
                    prop::option::of(hostile_text(2)),
                )
            })
            .prop_map(
                |(values, parts, arbitration, line2, connector, icon)| ModulePlan {
                    name: String::new(),
                    when: ModuleWhen::default(),
                    values: (0..values)
                        .map(|index| ValuePlan {
                            name: format!("v{index}"),
                            candidates: Vec::new(),
                        })
                        .collect(),
                    format: Template(parts),
                    appearance: ModuleAppearance {
                        icon,
                        style: Style::new().fg(Color::Green),
                        connector,
                    },
                    arbitration: arbitration.map(|(group, priority)| Arbitration {
                        group: group.to_owned(),
                        priority,
                    }),
                    slot: if line2 {
                        ModuleSlot::Line2
                    } else {
                        ModuleSlot::Line1
                    },
                },
            )
    }

    /// A `Clone` stand-in for [`Observation`], whose failure is not `Clone`.
    #[derive(Debug, Clone)]
    enum Seen<T> {
        Ready(T),
        Pending,
        Missing,
        Failed,
    }

    impl<T> Seen<T> {
        fn observe(self) -> Observation<T> {
            match self {
                Self::Ready(value) => Observation::Ready(value),
                Self::Pending => Observation::Pending,
                Self::Missing => Observation::Missing,
                Self::Failed => Observation::Failed(AcquireError::TimedOut),
            }
        }
    }

    type Snapshot = Vec<(Seen<bool>, Vec<Seen<String>>)>;

    fn observe(snapshot: Snapshot) -> Vec<ModuleObservation> {
        snapshot
            .into_iter()
            .map(|(condition, values)| ModuleObservation {
                condition: condition.observe(),
                values: values.into_iter().map(Seen::observe).collect(),
            })
            .collect()
    }

    fn seen<T: std::fmt::Debug + Clone>(
        ready: impl Strategy<Value = T>,
    ) -> impl Strategy<Value = Seen<T>> {
        prop_oneof![
            4 => ready.prop_map(Seen::Ready),
            1 => Just(Seen::Pending),
            1 => Just(Seen::Missing),
            1 => Just(Seen::Failed),
        ]
    }

    /// A plan of up to six modules, each with one observation snapshot.
    fn modules() -> impl Strategy<Value = (ConfigPlan, Snapshot)> {
        prop::collection::vec(module(), 0..6)
            .prop_flat_map(|modules| {
                let observed = modules
                    .iter()
                    .map(|module| {
                        (
                            seen(prop::bool::weighted(0.9)),
                            prop::collection::vec(seen(bulky_text()), module.values.len()),
                        )
                    })
                    .collect::<Vec<_>>();
                (Just(modules), observed)
            })
            .prop_map(|(modules, observed)| {
                let plan = ConfigPlan {
                    modules,
                    ..ConfigPlan::default()
                };
                (plan, observed)
            })
    }

    /// The format evaluation rule restated: a required value that is not ready
    /// hides the format, an optional section that cannot be shown is omitted,
    /// and text beyond the module bound is not shown.
    fn reference_format(parts: &[FormatPart], values: &[Observation<String>]) -> Option<String> {
        let mut text = String::new();
        for part in parts {
            match part {
                FormatPart::Literal(literal) => text.push_str(literal),
                FormatPart::Value(index) => match values.get(*index)? {
                    Observation::Ready(value) => text.push_str(value),
                    _ => return None,
                },
                FormatPart::Optional(parts) => {
                    text.push_str(&reference_format(parts, values).unwrap_or_default());
                }
            }
        }
        (text.len() <= MAX_MODULE_TEXT_BYTES).then_some(text)
    }

    /// The arbitration rule restated: among displayable modules, each group
    /// keeps its lowest priority, and the earliest declaration on a tie.
    fn reference_visible(plan: &ConfigPlan, observed: &[ModuleObservation]) -> Vec<usize> {
        let displayable = plan
            .modules
            .iter()
            .zip(observed)
            .enumerate()
            .filter(|(_, (module, observed))| {
                matches!(observed.condition, Observation::Ready(true))
                    && reference_format(&module.format.0, &observed.values)
                        .is_some_and(|text| !text.is_empty())
            })
            .map(|(index, (module, _))| (index, module))
            .collect::<Vec<_>>();
        displayable
            .iter()
            .filter(|(index, module)| {
                module.arbitration.as_ref().is_none_or(|rule| {
                    displayable
                        .iter()
                        .filter_map(|(other, module)| {
                            let other_rule = module.arbitration.as_ref()?;
                            (other_rule.group == rule.group)
                                .then_some((other_rule.priority, *other))
                        })
                        .min()
                        == Some((rule.priority, *index))
                })
            })
            .map(|(index, _)| *index)
            .collect()
    }

    fn view_input() -> impl Strategy<Value = (String, usize, String, Option<String>)> {
        (
            bulky_text(),
            prop_oneof![0..120usize, Just(usize::from(u16::MAX))],
            prop_oneof![
                Just("main".to_owned()),
                Just("vicmd".to_owned()),
                hostile_text(8)
            ],
            prop::option::of(bulky_text()),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn rendered_lines_fit_the_columns_and_the_response_frame(
            (plan, snapshot) in modules(),
            (directory, cols, keymap, branch) in view_input(),
            glyph in bulky_text(),
        ) {
            let observed = observe(snapshot);
            let mut plan = plan;
            plan.view.character.glyph = glyph;
            let git = branch.map(|branch| GitStatus { branch: Some(branch), ..GitStatus::default() });
            let output = render(&plan, &ViewInput {
                directory: &directory,
                git: git.as_ref(),
                modules: &observed,
                cols,
                keymap: &keymap,
                duration_ms: Some(u64::MAX),
                time: Some((23, 59, 59)),
                ..input()
            });
            for line in [&output.left1, &output.left2] {
                let displayed = displayed_prompt(line);
                prop_assert!(displayed.is_some(), "{line:?} has an unexpected escape");
                prop_assert!(displayed.unwrap_or_default().width() <= cols, "{line:?} exceeds {cols}");
                if cols == 0 {
                    prop_assert!(line.is_empty());
                }
            }
            let frame = capsule_protocol::session::response(u64::MAX, &output.left1, &output.left2, true);
            prop_assert!(frame.is_ok(), "{frame:?}");
        }

        #[test]
        fn format_evaluation_matches_the_reference_rule((plan, snapshot) in modules()) {
            let observed = observe(snapshot);
            for (module, observed) in plan.modules.iter().zip(&observed) {
                prop_assert_eq!(
                    evaluate_format(&module.format.0, &observed.values),
                    reference_format(&module.format.0, &observed.values)
                );
            }
        }

        #[test]
        fn arbitration_selects_the_lowest_priority_per_group((plan, snapshot) in modules()) {
            let observed = observe(snapshot);
            let visible = visible_modules(&plan, &observed)
                .into_iter()
                .filter_map(|(module, _)| plan.modules.iter().position(|candidate| std::ptr::eq(candidate, module)))
                .collect::<Vec<_>>();
            prop_assert_eq!(visible, reference_visible(&plan, &observed));
        }
    }
}
