//! Rendering pipeline for composing prompt lines from module outputs.
//!
//! The pipeline has three stages:
//! - **Style** ([`style`]): ANSI color codes wrapped in zsh `%{..%}` escapes
//! - **Layout** ([`layout`]): display width calculation and truncation
//! - **Composition** ([`compose`]): arranging segments into right-padded prompt lines

pub mod layout;
pub mod style;

pub use layout::{display_width, truncate};
pub use style::{Color, Style};

/// Composed prompt output ready for the wire protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptLines {
    /// Info line (line 1 of the prompt).
    pub left1: String,
    /// Input line (line 2 of the prompt).
    pub left2: String,
}

/// Compose module output segments into prompt lines.
///
/// `info_left` and `info_right` form the info line (line 1). Right segments
/// are right-aligned with space padding. When total width exceeds `cols`,
/// the first left segment (directory) is truncated before right segments
/// are dropped.
///
/// `input_left` segments form the input line (line 2), joined with spaces.
#[must_use]
pub fn compose(
    info_left: &[&str],
    info_right: &[&str],
    input_left: &[&str],
    cols: usize,
) -> PromptLines {
    PromptLines {
        left1: compose_info_line(info_left, info_right, cols),
        left2: join_non_empty(input_left),
    }
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

fn join_non_empty(parts: &[&str]) -> String {
    let mut out = String::new();
    for part in parts.iter().copied().filter(|s| !s.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

fn compose_info_line(left: &[&str], right: &[&str], cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }

    let left_joined = join_non_empty(left);
    let right_joined = join_non_empty(right);
    let left_w = display_width(&left_joined);
    let right_w = display_width(&right_joined);

    // Case 1: everything fits
    if fits(left_w, right_w, cols) {
        return padded(&left_joined, left_w, &right_joined, right_w, cols);
    }

    // Case 2: truncate directory (first left segment), keep all right
    if let Some(line) = try_truncate_directory(left, &right_joined, right_w, cols) {
        return line;
    }

    // Case 3: drop right segments from the end, one at a time
    for drop in 1..=right.len() {
        let remaining_right = &right[..right.len() - drop];
        let rj = join_non_empty(remaining_right);
        let rw = display_width(&rj);

        if fits(left_w, rw, cols) {
            return padded(&left_joined, left_w, &rj, rw, cols);
        }

        // Also try truncating directory with fewer right segments
        if let Some(line) = try_truncate_directory(left, &rj, rw, cols) {
            return line;
        }
    }

    // Case 4: nothing fits — truncate left to cols
    truncate(&left_joined, cols)
}

fn fits(left_w: usize, right_w: usize, cols: usize) -> bool {
    let gap = usize::from(left_w > 0 && right_w > 0);
    left_w + gap + right_w <= cols
}

fn padded(left: &str, left_w: usize, right: &str, right_w: usize, cols: usize) -> String {
    let padding = cols.saturating_sub(left_w + right_w);
    let mut out = String::with_capacity(left.len() + padding + right.len());
    out.push_str(left);
    for _ in 0..padding {
        out.push(' ');
    }
    out.push_str(right);
    out
}

fn try_truncate_directory(
    left: &[&str],
    right_joined: &str,
    right_w: usize,
    cols: usize,
) -> Option<String> {
    if left.is_empty() {
        return None;
    }

    let other_left = join_non_empty(&left[1..]);
    let other_w = display_width(&other_left);

    // Overhead: other left segments + separator (if any) + min gap + right
    let sep = usize::from(other_w > 0);
    let gap = usize::from(right_w > 0);
    let overhead = other_w + sep + gap + right_w;

    if overhead >= cols {
        return None;
    }

    let available = cols - overhead;
    let dir_truncated = truncate(left[0], available);

    let mut new_left = dir_truncated;
    if !other_left.is_empty() {
        new_left.push(' ');
        new_left.push_str(&other_left);
    }

    let new_left_w = display_width(&new_left);
    Some(padded(&new_left, new_left_w, right_joined, right_w, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- compose_info_line: right-alignment with padding --

    #[test]
    fn test_render_compose_fits_with_padding() {
        let line = compose_info_line(&["~/proj", "main"], &["rust", "14:30"], 40);
        assert_eq!(display_width(&line), 40);
        assert!(line.starts_with("~/proj main"));
        assert!(line.ends_with("rust 14:30"));
    }

    #[test]
    fn test_render_compose_right_aligned() {
        let line = compose_info_line(&["left"], &["right"], 20);
        assert_eq!(display_width(&line), 20);
        assert!(line.starts_with("left"));
        assert!(line.ends_with("right"));
    }

    #[test]
    fn test_render_compose_exact_fit() {
        // "ab" (2) + " " (1) + "cd" (2) = 5
        let line = compose_info_line(&["ab"], &["cd"], 5);
        assert_eq!(line, "ab cd");
    }

    // -- compose_info_line: directory truncation --

    #[test]
    fn test_render_compose_truncates_directory() {
        let line = compose_info_line(&["very/long/directory/path/here"], &["right"], 15);
        assert!(display_width(&line) <= 15);
        assert!(
            line.contains('\u{2026}'),
            "should contain ellipsis: {line:?}"
        );
        assert!(
            line.ends_with("right"),
            "right should be preserved: {line:?}"
        );
    }

    #[test]
    fn test_render_compose_truncates_directory_preserves_other_left() {
        let line = compose_info_line(&["very/long/directory/path", "main"], &["rust"], 20);
        assert!(display_width(&line) <= 20);
        assert!(
            line.contains("main"),
            "other left segments preserved: {line:?}"
        );
        assert!(line.ends_with("rust"), "right preserved: {line:?}");
    }

    // -- compose_info_line: dropping right segments --

    #[test]
    fn test_render_compose_drops_right_segments() {
        // Right is too wide for cols, left is short
        let line = compose_info_line(&["dir"], &["wide-seg-a", "wide-seg-b"], 15);
        assert!(display_width(&line) <= 15);
        // At least directory should be present
        assert!(
            line.contains("dir"),
            "directory should be preserved: {line:?}"
        );
    }

    // -- compose_info_line: edge cases --

    #[test]
    fn test_render_compose_empty_right() {
        let line = compose_info_line(&["hello"], &[], 20);
        assert_eq!(display_width(&line), 20);
        assert!(line.starts_with("hello"));
    }

    #[test]
    fn test_render_compose_empty_left() {
        let line = compose_info_line(&[], &["right"], 20);
        assert_eq!(display_width(&line), 20);
        assert!(line.ends_with("right"));
    }

    #[test]
    fn test_render_compose_both_empty() {
        let line = compose_info_line(&[], &[], 20);
        assert_eq!(display_width(&line), 20);
    }

    #[test]
    fn test_render_compose_zero_cols() {
        let line = compose_info_line(&["hello"], &["world"], 0);
        assert_eq!(line, "");
    }

    // -- compose_info_line: styled content --

    #[test]
    fn test_render_compose_with_styled_segments() {
        let styled_left = Style::new().fg(Color::Cyan).paint("~/project");
        let styled_right = Style::new().fg(Color::Blue).paint("rust");

        let line = compose_info_line(&[&styled_left], &[&styled_right], 30);
        assert_eq!(display_width(&line), 30);
    }

    // -- compose: full prompt --

    #[test]
    fn test_render_compose_full_prompt() {
        let result = compose(&["~/proj", "main"], &["rust", "14:30"], &["❯"], 40);
        assert_eq!(display_width(&result.left1), 40);
        assert_eq!(result.left2, "❯");
    }

    #[test]
    fn test_render_compose_input_line_joins_segments() {
        let result = compose(&["dir"], &[], &["✗", "130"], 40);
        assert_eq!(result.left2, "✗ 130");
    }

    #[test]
    fn test_render_compose_input_line_skips_empty() {
        let result = compose(&["dir"], &[], &["", "❯", ""], 40);
        assert_eq!(result.left2, "❯");
    }
}
