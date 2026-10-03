//! Built-in display functions consume only values supplied by the session.

use super::{ViewInput, decorate, text::StyledText};
use crate::{git::GitStatus, plan::ViewConfig, render::style::Style};

pub(super) fn directory(config: &ViewConfig, input: &ViewInput<'_>) -> StyledText {
    let mut content = StyledText::new(input.directory, config.directory.prompt_style());
    if input.read_only {
        content.push(" ", Style::new());
        content.push("\u{f023}", config.directory.read_only_prompt_style());
    }
    content
}

pub(super) fn git(config: &ViewConfig, status: &GitStatus) -> Option<StyledText> {
    let style = &config.git;
    let mut content = StyledText::default();
    if let Some(branch) = &status.branch {
        content.push(branch, style.prompt_style());
    } else if let Some(oid) = &status.head_oid {
        let prefix: String = oid.chars().take(7).collect();
        if !prefix.is_empty() {
            content.push("HEAD ", style.prompt_style());
            content.push(&format!("({prefix})"), style.detached_hash_prompt_style());
        }
    }
    if let Some(state) = status.state {
        if !content.is_empty() {
            content.push(" ", Style::new());
        }
        let state = match (state.step, state.total) {
            (Some(step), Some(total)) => format!("({} {step}/{total})", state.state),
            _ => format!("({})", state.state),
        };
        content.push(&state, style.state_prompt_style());
    }
    let indicators = indicators(status);
    if !indicators.is_empty() {
        if !content.is_empty() {
            content.push(" ", Style::new());
        }
        content.push(&format!("[{indicators}]"), style.indicator_prompt_style());
    }
    if content.is_empty() {
        return None;
    }
    Some(decorate(
        content,
        Some((&style.connector, config.connectors.prompt_style())),
        Some((&style.icon, style.prompt_style())),
    ))
}

fn indicators(status: &GitStatus) -> String {
    let mut indicators = String::new();
    for (count, symbol) in [
        (status.conflicted, '='),
        (status.stashed, '$'),
        (status.deleted, '✘'),
        (status.renamed, '»'),
        (status.modified, '!'),
        (status.staged, '+'),
        (status.untracked, '?'),
    ] {
        if count > 0 {
            indicators.push(symbol);
        }
    }
    match (status.ahead > 0, status.behind > 0) {
        (true, true) => indicators.push('⇕'),
        (true, false) => indicators.push('⇡'),
        (false, true) => indicators.push('⇣'),
        (false, false) => {}
    }
    indicators
}

pub(super) fn duration(config: &ViewConfig, duration_ms: Option<u64>) -> Option<StyledText> {
    let duration_ms = duration_ms?;
    if config.cmd_duration.disabled || duration_ms < config.cmd_duration.threshold_ms {
        return None;
    }
    Some(decorate(
        StyledText::new(
            &format_duration(duration_ms),
            config.cmd_duration.prompt_style(),
        ),
        Some((
            &config.cmd_duration.connector,
            config.connectors.prompt_style(),
        )),
        None,
    ))
}

fn format_duration(milliseconds: u64) -> String {
    use std::fmt::Write as _;

    let seconds = milliseconds / 1000;
    let mut formatted = String::new();
    for (count, unit) in [
        (seconds / 86_400, 'd'),
        ((seconds % 86_400) / 3600, 'h'),
        ((seconds % 3600) / 60, 'm'),
    ] {
        if count > 0 {
            let _ = write!(formatted, "{count}{unit}");
        }
    }
    if formatted.is_empty() || !seconds.is_multiple_of(60) {
        let _ = write!(formatted, "{}s", seconds % 60);
    }
    formatted
}

pub(super) fn time(config: &ViewConfig, time: Option<(u8, u8, u8)>) -> Option<StyledText> {
    if config.time.disabled {
        return None;
    }
    let (hour, minute, second) = time?;
    let content = if config.time.show_seconds() {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{hour:02}:{minute:02}")
    };
    Some(decorate(
        StyledText::new(&content, config.time.prompt_style()),
        Some((&config.time.connector, config.connectors.prompt_style())),
        None,
    ))
}

pub(super) fn character(config: &ViewConfig, input: &ViewInput<'_>) -> StyledText {
    let character = &config.character;
    let exit_style = if input.last_exit_code == 0 {
        character.success_prompt_style()
    } else {
        character.error_prompt_style()
    };
    if input.keymap == "vicmd" {
        let style = character
            .vicmd
            .style
            .as_ref()
            .map_or(exit_style, |style| style.resolve(Style::new()));
        StyledText::new(&character.vicmd.glyph, style)
    } else {
        StyledText::new(&character.glyph, exit_style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_preserves_threshold_units_and_exact_boundaries() {
        for (milliseconds, expected) in [
            (0, "0s"),
            (1999, "1s"),
            (2000, "2s"),
            (3500, "3s"),
            (65_000, "1m5s"),
            (120_000, "2m"),
            (3_600_000, "1h"),
            (3_661_000, "1h1m1s"),
            (86_400_000, "1d"),
            (90_061_000, "1d1h1m1s"),
        ] {
            assert_eq!(format_duration(milliseconds), expected);
        }
    }

    #[test]
    fn git_indicator_order_and_divergence_are_preserved() {
        let mut status = GitStatus {
            staged: 1,
            modified: 1,
            untracked: 1,
            conflicted: 1,
            stashed: 1,
            deleted: 1,
            renamed: 1,
            ahead: 1,
            behind: 1,
            ..GitStatus::default()
        };
        assert_eq!(indicators(&status), "=$✘»!+?⇕");
        status.behind = 0;
        assert_eq!(indicators(&status), "=$✘»!+?⇡");
        status.ahead = 0;
        status.behind = 1;
        assert_eq!(indicators(&status), "=$✘»!+?⇣");
        assert_eq!(indicators(&GitStatus::default()), "");
    }
}
