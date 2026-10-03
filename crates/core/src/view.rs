//! Pure prompt composition from a compiled plan and display observations.
//!
//! Keep display text and styles separate through layout. Serialize ANSI and zsh
//! percent escapes only after the complete grapheme-aware line fits its width.

mod builtins;
mod text;

use std::collections::HashMap;

use self::text::StyledText;
use crate::{
    git::GitStatus,
    plan::{ConfigPlan, FormatPart, ModuleObservation, ModulePlan, ModuleSlot, Observation},
    render::style::Style,
};

// Prevent repeated references in a short format from multiplying bounded source
// values into an unbounded intermediate allocation.
const MAX_MODULE_TEXT_BYTES: usize = 64 * 1024;

/// Inputs already acquired by the session; rendering performs no I/O.
#[derive(Debug)]
pub struct ViewInput<'a> {
    /// Repository-relative or home-abbreviated directory display text.
    pub directory: &'a str,
    /// Whether the acquired directory permissions indicate read-only access.
    pub read_only: bool,
    /// Git status from the worker-selected display snapshot, when available.
    pub git: Option<&'a GitStatus>,
    /// Observations aligned with the plan's module and value declaration order.
    pub modules: &'a [ModuleObservation],
    /// Available terminal columns for each prompt line.
    pub cols: usize,
    /// Last executed command's exit status.
    pub last_exit_code: i32,
    /// Last executed command's elapsed time.
    pub duration_ms: Option<u64>,
    /// Current zsh keymap; `vicmd` selects the command-mode character.
    pub keymap: &'a str,
    /// Current local hour, minute, and second, supplied by the session.
    pub time: Option<(u8, u8, u8)>,
}

/// Two prompt lines after width adjustment and zsh serialization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptLines {
    /// Information line.
    pub left1: String,
    /// Input line.
    pub left2: String,
}

/// Render one immutable observation snapshot without filesystem or command access.
///
/// The shell must assign the returned data directly when `PROMPT_SUBST` is off,
/// or use a one-level parameter reference when it is on. Percent escapes are
/// quoted here; dollar signs, backticks, and backslashes remain literal data.
#[must_use]
pub fn render(plan: &ConfigPlan, input: &ViewInput<'_>) -> PromptLines {
    let config = &plan.view;
    let connector_style = config.connectors.prompt_style();
    let custom = visible_modules(plan, input.modules);
    let mut line1 = Vec::new();
    let mut line2 = Vec::new();

    if !config.directory.disabled {
        line1.push(builtins::directory(config, input));
    }
    if !config.git.disabled
        && let Some(status) = input.git
        && let Some(git) = builtins::git(config, status)
    {
        line1.push(git);
    }
    for (module, content) in custom {
        let segment = decorate(
            StyledText::new(&content, module.appearance.style),
            module
                .appearance
                .connector
                .as_deref()
                .map(|word| (word, connector_style)),
            module
                .appearance
                .icon
                .as_deref()
                .map(|icon| (icon, module.appearance.style)),
        );
        match module.slot {
            ModuleSlot::Line1 => line1.push(segment),
            ModuleSlot::Line2 => line2.push(segment),
        }
    }
    if let Some(duration) = builtins::duration(config, input.duration_ms) {
        line1.push(duration);
    }
    if let Some(time) = builtins::time(config, input.time) {
        line2.push(time);
    }
    if !config.character.disabled {
        line2.push(builtins::character(config, input));
    }
    PromptLines {
        left1: fit_line(&line1, input.cols).to_zsh(config.color_map),
        left2: fit_line(&line2, input.cols).to_zsh(config.color_map),
    }
}

fn visible_modules<'a>(
    plan: &'a ConfigPlan,
    observations: &[ModuleObservation],
) -> Vec<(&'a ModulePlan, String)> {
    let eligible: Vec<_> = plan
        .modules
        .iter()
        .zip(observations)
        .enumerate()
        .filter_map(|(index, (module, observed))| {
            if !matches!(observed.condition, Observation::Ready(true)) {
                return None;
            }
            let content = evaluate_format(&module.format.0, &observed.values)?;
            (!content.is_empty()).then_some((index, module, content))
        })
        .collect();

    // Choose once across both display slots and every acquisition source.
    // Insertion on a strictly lower priority preserves declaration-order ties.
    let mut winners = HashMap::<&str, (u32, usize)>::new();
    for (index, module, _) in &eligible {
        if let Some(rule) = &module.arbitration {
            let winner = winners
                .entry(&rule.group)
                .or_insert((rule.priority, *index));
            if rule.priority < winner.0 {
                *winner = (rule.priority, *index);
            }
        }
    }
    eligible
        .into_iter()
        .filter_map(|(index, module, content)| {
            let selected = module.arbitration.as_ref().is_none_or(|rule| {
                winners
                    .get(rule.group.as_str())
                    .is_some_and(|winner| winner.1 == index)
            });
            selected.then_some((module, content))
        })
        .collect()
}

fn evaluate_format(parts: &[FormatPart], values: &[Observation<String>]) -> Option<String> {
    let mut text = String::new();
    for part in parts {
        match part {
            FormatPart::Literal(literal) => append_bounded(&mut text, literal)?,
            FormatPart::Value(index) => match values.get(*index)? {
                Observation::Ready(value) => append_bounded(&mut text, value)?,
                Observation::Pending | Observation::Missing | Observation::Failed(_) => {
                    return None;
                }
            },
            FormatPart::Optional(parts) => {
                if let Some(optional) = evaluate_format(parts, values) {
                    append_bounded(&mut text, &optional)?;
                }
            }
        }
    }
    Some(text)
}

fn append_bounded(output: &mut String, text: &str) -> Option<()> {
    if text.len() > MAX_MODULE_TEXT_BYTES.saturating_sub(output.len()) {
        return None;
    }
    output.push_str(text);
    Some(())
}

fn decorate(
    content: StyledText,
    connector: Option<(&str, Style)>,
    icon: Option<(&str, Style)>,
) -> StyledText {
    let mut parts = Vec::with_capacity(3);
    for (text, style) in connector.into_iter().chain(icon) {
        if !text.is_empty() {
            parts.push(StyledText::new(text, style));
        }
    }
    parts.push(content);
    StyledText::join(&parts)
}

fn fit_line(segments: &[StyledText], cols: usize) -> StyledText {
    if cols == 0 {
        return StyledText::default();
    }
    let mut remaining = segments;
    while let Some((first, rest)) = remaining.split_first() {
        let joined = StyledText::join(remaining);
        if joined.width() <= cols {
            return joined.truncate(cols);
        }
        let rest = StyledText::join(rest);
        let overhead = rest.width() + usize::from(!rest.is_empty());
        if overhead < cols {
            return StyledText::join(&[first.truncate(cols - overhead), rest]).truncate(cols);
        }
        remaining = &remaining[..remaining.len() - 1];
    }
    StyledText::default()
}

#[cfg(test)]
mod tests;
