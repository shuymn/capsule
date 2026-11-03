use super::*;
use crate::{
    git::{GitOperationState, GitState},
    plan::{StyleConfig, TimeFormat, ViewConfig},
    render::style::{Color, ColorMap},
    test_utils::{contains_style_sequence, style_sequence},
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
    let no_room = fit_line(&[segment("dir"), segment("main")], 5);
    assert_eq!(no_room.to_zsh(ColorMap::default()), "dir");
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
    config.character.vicmd.style = Some(StyleConfig::fg(Color::Blue));
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
fn builtins_keep_two_lines_connectors_and_independent_styles() {
    let mut plan = ConfigPlan::default();
    plan.view.time.disabled = false;
    plan.view.time.format = TimeFormat::WithoutSeconds;
    plan.view.connectors.style = StyleConfig::fg(Color::Blue);
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
    let styled =
        |codes: &[u8], text: &str| format!("%{{{}%}}{text}%{{\x1b[0m%}}", style_sequence(codes));
    let left1 = [
        styled(&[1, 36], "project"),
        styled(&[31], "\u{f023}"),
        styled(&[34], "on"),
        styled(&[1, 35], "\u{f418}"),
        styled(&[1, 35], "HEAD ") + &styled(&[1, 32], "(abcdef0)"),
        styled(&[1, 33], "(REBASING 2/5)"),
        styled(&[1, 31], "[!]"),
        styled(&[34], "took"),
        styled(&[1, 33], "1m5s"),
    ];
    assert_eq!(prompt.left1, left1.join(" "));
    let left2 = [
        styled(&[34], "at"),
        styled(&[1, 33], "14:05"),
        styled(&[1, 32], "❯"),
    ];
    assert_eq!(prompt.left2, left2.join(" "));
}

#[test]
fn builtins_disabled_and_unavailable_values_stay_absent() {
    let mut plan = ConfigPlan::default();
    let threshold = plan.view.cmd_duration.threshold_ms;
    assert!(builtins::duration(&plan.view, Some(threshold)).is_some());
    assert!(builtins::duration(&plan.view, Some(threshold - 1)).is_none());
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
fn partial_git_status_has_no_leading_separator() {
    let config = ViewConfig::default();
    let git = |status: GitStatus| builtins::git(&config, &status).map(|text| text.plain_text());
    let state = GitOperationState {
        state: GitState::Merge,
        step: None,
        total: None,
    };
    assert_eq!(
        git(GitStatus {
            state: Some(state),
            ..GitStatus::default()
        }),
        Some("on \u{f418} (MERGING)".to_owned())
    );
    assert_eq!(
        git(GitStatus {
            untracked: 1,
            ..GitStatus::default()
        }),
        Some("on \u{f418} [?]".to_owned())
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

mod properties {
    use std::collections::BTreeSet;

    use proptest::{prelude::*, strategy::LazyJust};
    use unicode_width::UnicodeWidthStr as _;

    use super::*;
    use crate::{
        acquire::AcquireError,
        plan::{Arbitration, ModuleAppearance, ModuleWhen, Template, ValuePlan},
        test_utils::{bulky_text, cols, displayed_prompt, format_parts, hostile_text},
    };

    fn module() -> impl Strategy<Value = ModulePlan> {
        (0..3usize)
            .prop_flat_map(|values| {
                (
                    Just(values),
                    format_parts(values, 3),
                    prop::option::of((prop::sample::select(vec!["a", "b"]), 0..3u32)),
                    prop::bool::ANY,
                    prop::option::of(hostile_text(4)),
                    prop::option::of(hostile_text(2)),
                    prop::sample::select(vec![Style::new(), Style::new().fg(Color::Green).bold()]),
                )
            })
            .prop_map(
                |(values, parts, arbitration, line2, connector, icon, style)| ModulePlan {
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
                        style,
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

    fn observation<T: std::fmt::Debug>(
        ready: impl Strategy<Value = T>,
    ) -> impl Strategy<Value = Observation<T>> {
        prop_oneof![
            4 => ready.prop_map(Observation::Ready),
            1 => LazyJust::new(|| Observation::Pending),
            1 => LazyJust::new(|| Observation::Missing),
            1 => LazyJust::new(|| Observation::Failed(AcquireError::TimedOut)),
        ]
    }

    /// Up to six modules, named by declaration index, with one observation
    /// snapshot each.
    fn modules() -> impl Strategy<Value = (ConfigPlan, Vec<ModuleObservation>)> {
        prop::collection::vec(module(), 0..6)
            .prop_flat_map(|modules| {
                let observed = modules
                    .iter()
                    .map(|module| {
                        (
                            observation(prop::bool::weighted(0.9)),
                            prop::collection::vec(observation(bulky_text()), module.values.len()),
                        )
                            .prop_map(|(condition, values)| ModuleObservation { condition, values })
                    })
                    .collect::<Vec<_>>();
                (Just(modules), observed)
            })
            .prop_map(|(mut modules, observed)| {
                for (index, module) in modules.iter_mut().enumerate() {
                    module.name = format!("m{index}");
                }
                let plan = ConfigPlan {
                    modules,
                    ..ConfigPlan::default()
                };
                (plan, observed)
            })
    }

    /// A copy of an observation; `Observation` is not `Clone` because a failure
    /// can hold an I/O error.
    fn duplicate(observed: &ModuleObservation) -> ModuleObservation {
        fn copy<T: Clone>(observation: &Observation<T>) -> Observation<T> {
            match observation {
                Observation::Ready(value) => Observation::Ready(value.clone()),
                Observation::Pending => Observation::Pending,
                Observation::Missing => Observation::Missing,
                Observation::Failed(_) => Observation::Failed(AcquireError::TimedOut),
            }
        }
        ModuleObservation {
            condition: copy(&observed.condition),
            values: observed.values.iter().map(copy).collect(),
        }
    }

    fn visible_names(plan: &ConfigPlan, observed: &[ModuleObservation]) -> Vec<String> {
        visible_modules(plan, observed)
            .into_iter()
            .map(|(module, _)| module.name.clone())
            .collect()
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
    fn reference_visible(plan: &ConfigPlan, observed: &[ModuleObservation]) -> Vec<String> {
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
            .map(|(_, module)| module.name.clone())
            .collect()
    }

    proptest! {
        /// Covers: T01, T07
        #[test]
        fn rendered_lines_fit_the_columns_and_the_response_frame(
            (mut plan, observed) in modules(),
            directory in bulky_text(),
            cols in cols(120),
            keymap in prop_oneof![Just("main".to_owned()), Just("vicmd".to_owned()), hostile_text(8)],
            branch in prop::option::of(bulky_text()),
            glyph in bulky_text(),
        ) {
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
                prop_assert!(
                    displayed_prompt(line).is_some_and(|displayed| displayed.width() <= cols),
                    "{line:?} is not literal data within {cols} columns"
                );
                prop_assert!(cols != 0 || line.is_empty());
            }
            let frame = capsule_protocol::session::response(u64::MAX, &output.left1, &output.left2, true);
            prop_assert!(frame.is_ok(), "{frame:?}");
        }

        #[test]
        fn format_evaluation_matches_the_reference_rule((plan, observed) in modules()) {
            for (module, observed) in plan.modules.iter().zip(&observed) {
                prop_assert_eq!(
                    evaluate_format(&module.format.0, &observed.values),
                    reference_format(&module.format.0, &observed.values)
                );
            }
        }

        #[test]
        fn arbitration_selects_the_lowest_priority_per_group((plan, observed) in modules()) {
            prop_assert_eq!(visible_names(&plan, &observed), reference_visible(&plan, &observed));
        }

        /// Without priority ties, the winners do not depend on declaration order.
        #[test]
        fn arbitration_without_ties_ignores_declaration_order(
            (plan, observed) in modules(),
            shuffled in Just((0..6).collect::<Vec<usize>>()).prop_shuffle(),
        ) {
            // `modules()` declares at most six modules; keep the indices in range.
            let order = shuffled.into_iter().filter(|index| *index < plan.modules.len()).collect::<Vec<_>>();
            let rules = plan.modules.iter().filter_map(|module| module.arbitration.as_ref());
            let distinct = rules.clone().map(|rule| (&rule.group, rule.priority)).collect::<BTreeSet<_>>();
            prop_assume!(distinct.len() == rules.count());
            let reordered = ConfigPlan {
                modules: order.iter().map(|index| plan.modules[*index].clone()).collect(),
                ..ConfigPlan::default()
            };
            let reordered_observed = order.iter().map(|index| duplicate(&observed[*index])).collect::<Vec<_>>();
            let before = visible_names(&plan, &observed).into_iter().collect::<BTreeSet<_>>();
            let after = visible_names(&reordered, &reordered_observed).into_iter().collect::<BTreeSet<_>>();
            prop_assert_eq!(before, after);
        }
    }
}
