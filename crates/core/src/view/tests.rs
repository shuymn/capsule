use super::*;
use crate::{
    git::{GitOperationState, GitState},
    plan::{StyleConfig, TimeFormat, ViewConfig},
    render::style::{Color, ColorMap},
    test_utils::{contains_style_sequence, displayed_prompt, styled},
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

#[test]
fn custom_modules_render_on_their_slot_in_declaration_order_between_builtins()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = ConfigPlan::parse(
        r#"
schema_version = 2
[directory]
disabled = true
[character]
disabled = true
[[module]]
name = "first"
values = { value = [{ env = "A" }] }
[[module]]
name = "second"
slot = "line2"
values = { value = [{ env = "B" }] }
[[module]]
name = "third"
values = { value = [{ env = "C" }] }
"#,
    )?;
    let observations = [observed("first"), observed("second"), observed("third")];
    let status = GitStatus {
        branch: Some("main".into()),
        ..GitStatus::default()
    };
    let output = render(
        &plan,
        &ViewInput {
            git: Some(&status),
            modules: &observations,
            duration_ms: Some(65_000),
            ..input()
        },
    );
    assert_eq!(
        displayed_prompt(&output.left1).as_deref(),
        Some("on \u{f418} main first third took 1m5s")
    );
    assert_eq!(displayed_prompt(&output.left2).as_deref(), Some("second"));
    Ok(())
}

mod properties {
    use std::collections::BTreeSet;

    use proptest::{prelude::*, strategy::LazyJust};

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

    /// The characters of `test_utils::HOSTILE` and the built-in glyphs that
    /// zsh's `wcwidth` in a UTF-8 locale measures as `unicode-width` does,
    /// besides printable ASCII. architecture.md lists where they disagree.
    const LIBC_AGREES: &str = "日本\u{301}\u{200b}\u{200d}\u{644}\u{627}…❯\u{f418}";

    fn libc_agrees(text: &str) -> bool {
        text.chars().all(|character| {
            character == ' ' || character.is_ascii_graphic() || LIBC_AGREES.contains(character)
        })
    }

    /// Display data mixing width-stable text, hostile characters, and code that
    /// would expand a parameter or append to `$MARKER` if zsh evaluated it.
    fn zsh_input() -> impl Strategy<Value = String> {
        let stable = LIBC_AGREES
            .chars()
            .chain("a %$`\\{}".chars())
            .collect::<Vec<_>>();
        let code = prop::sample::select(vec![
            "$(print x >> $MARKER)",
            "`print x >> $MARKER`",
            "%{$(print x >> $MARKER)%}",
            "${MARKER::=x}$HOME",
            "\\",
        ]);
        let part = prop_oneof![
            prop::collection::vec(prop::sample::select(stable), 0..24).prop_map(String::from_iter),
            code.prop_map(str::to_owned),
            hostile_text(8),
        ];
        prop::collection::vec(part, 0..4).prop_map(|parts| parts.concat())
    }

    /// What zsh shows for `lines`, run in `cwd`: the prompt with its styles
    /// removed as expanded with `PROMPT_SUBST` through Capsule's one-level
    /// reference, with it on and the text assigned directly, and with it off;
    /// then the `${(m)#}` width of each line of the last expansion.
    fn expand_in_zsh(
        lines: &PromptLines,
        cwd: &std::path::Path,
    ) -> Result<Vec<String>, TestCaseError> {
        let output = std::process::Command::new("zsh")
            .current_dir(cwd)
            .args([
                "-f",
                "-c",
                r#"
wide=日
for LC_ALL in C.UTF-8 en_US.UTF-8; do (( ${(m)#wide} == 2 )) && break; done
(( ${(m)#wide} == 2 )) || { print -u2 'no UTF-8 locale'; exit 1 }
setopt extendedglob
rendered=$LEFT1$'\n'$LEFT2
show() { shown=${1//$'\e'\[[0-9;]#m}; print -rn -- "$shown"$'\0' }
setopt promptsubst
print -v expanded -Pr -- '${rendered}'; show "$expanded"
print -v expanded -Pr -- "$rendered"; show "$expanded"
unsetopt promptsubst
print -v expanded -Pr -- "$rendered"; show "$expanded"
first=${shown%%$'\n'*} second=${shown#*$'\n'}
print -rn -- ${(m)#first}$'\0'${(m)#second}$'\0'
"#,
            ])
            .env("LEFT1", &lines.left1)
            .env("LEFT2", &lines.left2)
            .env("MARKER", cwd.join("marker"))
            .output()
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        if !output.status.success() {
            return Err(TestCaseError::fail(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        Ok(stdout.split_terminator('\0').map(str::to_owned).collect())
    }

    proptest! {
        /// zsh expands each rendered line to its display text, without running
        /// or expanding data, and measures the width Capsule fitted.
        ///
        /// Covers: T01
        #[test]
        fn zsh_expands_rendered_lines_literally_to_the_measured_width(
            directory in zsh_input(),
            branch in zsh_input(),
            glyph in zsh_input(),
            cols in cols(60),
        ) {
            let mut plan = ConfigPlan::default();
            plan.view.character.glyph = glyph;
            let git = GitStatus { branch: Some(branch), ..GitStatus::default() };
            let output = render(&plan, &ViewInput { directory: &directory, git: Some(&git), cols, ..input() });
            let [Some(first), Some(second)] = [&output.left1, &output.left2].map(|line| displayed_prompt(line)) else {
                return Err(TestCaseError::fail(format!("{output:?} is not literal data")));
            };
            let cwd = tempfile::tempdir().map_err(|error| TestCaseError::fail(error.to_string()))?;
            let shown = format!("{first}\n{second}");
            let zsh = expand_in_zsh(&output, cwd.path())?;
            prop_assert_eq!(zsh.len(), 5, "{:?}", zsh);
            prop_assert!(zsh[..3].iter().all(|expanded| *expanded == shown), "{zsh:?} does not show {shown:?}");
            for (line, width) in [&first, &second].into_iter().zip(&zsh[3..]) {
                if libc_agrees(line) {
                    prop_assert_eq!(width, &text::display_width(line).to_string(), "{:?}", line);
                }
            }
            let written = std::fs::read_dir(cwd.path()).map_err(|error| TestCaseError::fail(error.to_string()))?;
            prop_assert_eq!(written.count(), 0, "expanding data wrote files");
        }

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
                    displayed_prompt(line).is_some_and(|displayed| text::display_width(&displayed) <= cols),
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
