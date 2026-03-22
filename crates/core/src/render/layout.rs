//! Display width calculation and string truncation.
//!
//! Handles ANSI escape sequences, zsh `%{..%}` prompt escapes, and
//! CJK double-width characters.

use std::{iter::Peekable, str::Chars};

use unicode_width::UnicodeWidthChar;

/// Calculate the display width of a string, excluding ANSI escape sequences
/// and zsh `%{..%}` prompt escapes.
///
/// CJK characters are counted as 2 columns.
#[must_use]
pub fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            skip_csi(&mut chars);
        } else if ch == '%' {
            match chars.peek() {
                Some(&'{') => {
                    chars.next();
                    skip_zsh_escape(&mut chars);
                }
                Some(&'%') => {
                    chars.next(); // consume second '%'
                    width += 1; // %% → 1 column in zsh
                }
                _ => {
                    width += 1; // bare % (defensive fallback)
                }
            }
        } else {
            width += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }

    width
}

/// Truncate a string to fit within `max_width` display columns.
///
/// If truncation occurs, an ellipsis (`…`) is appended. ANSI escape sequences
/// and zsh `%{..%}` escapes encountered before the cut point are preserved.
/// An ANSI reset is appended when the truncated text contains escape sequences.
///
/// Returns the original string unchanged when it already fits.
#[must_use]
pub fn truncate(s: &str, max_width: usize) -> String {
    if display_width(s) <= max_width {
        return s.to_owned();
    }

    if max_width == 0 {
        return String::new();
    }

    // Reserve 1 column for the ellipsis character.
    let effective_max = max_width - 1;
    let mut result = String::with_capacity(s.len());
    let mut width = 0;
    let mut has_escapes = false;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            has_escapes = true;
            result.push(ch);
            if let Some(bracket) = chars.next() {
                result.push(bracket);
            }
            copy_csi(&mut chars, &mut result);
        } else if ch == '%' {
            match chars.peek() {
                Some(&'{') => {
                    has_escapes = true;
                    result.push(ch);
                    if let Some(brace) = chars.next() {
                        result.push(brace);
                    }
                    copy_zsh_escape(&mut chars, &mut result);
                }
                Some(&'%') => {
                    // %% displays as 1 column; keep the pair together.
                    if width + 1 > effective_max {
                        break;
                    }
                    width += 1;
                    result.push(ch);
                    if let Some(second) = chars.next() {
                        result.push(second);
                    }
                }
                _ => {
                    // bare % (defensive fallback)
                    if width + 1 > effective_max {
                        break;
                    }
                    width += 1;
                    result.push(ch);
                }
            }
        } else {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + char_width > effective_max {
                break;
            }
            width += char_width;
            result.push(ch);
        }
    }

    result.push('\u{2026}'); // …

    if has_escapes {
        result.push_str("%{\x1b[0m%}");
    }

    result
}

// ---------------------------------------------------------------------------
// ANSI CSI helpers
// ---------------------------------------------------------------------------

fn skip_csi(chars: &mut Peekable<Chars<'_>>) {
    for c in chars.by_ref() {
        if is_csi_final(c) {
            break;
        }
    }
}

fn copy_csi(chars: &mut Peekable<Chars<'_>>, out: &mut String) {
    for c in chars.by_ref() {
        out.push(c);
        if is_csi_final(c) {
            break;
        }
    }
}

const fn is_csi_final(c: char) -> bool {
    c.is_ascii() && matches!(c as u8, 0x40..=0x7E)
}

// ---------------------------------------------------------------------------
// zsh %{..%} helpers
// ---------------------------------------------------------------------------

fn skip_zsh_escape(chars: &mut Peekable<Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '%' && chars.peek() == Some(&'}') {
            chars.next();
            break;
        }
    }
}

fn copy_zsh_escape(chars: &mut Peekable<Chars<'_>>, out: &mut String) {
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '%' && chars.peek() == Some(&'}') {
            if let Some(close) = chars.next() {
                out.push(close);
            }
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- display_width --

    #[test]
    fn test_render_width_plain_ascii() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn test_render_width_empty() {
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn test_render_width_cjk() {
        assert_eq!(display_width("日本語"), 6);
    }

    #[test]
    fn test_render_width_mixed_ascii_cjk() {
        assert_eq!(display_width("hi世界"), 6); // 2 + 2×2
    }

    #[test]
    fn test_render_width_ansi_escape() {
        assert_eq!(display_width("\x1b[31mred\x1b[0m"), 3);
    }

    #[test]
    fn test_render_width_zsh_escape() {
        assert_eq!(display_width("%{\x1b[31m%}red%{\x1b[0m%}"), 3);
    }

    #[test]
    fn test_render_width_ansi_bold_color() {
        assert_eq!(display_width("\x1b[1;32mbold green\x1b[0m"), 10);
    }

    #[test]
    fn test_render_width_bare_percent() {
        // A bare '%' not followed by '{' should be counted as visible
        assert_eq!(display_width("100%"), 4);
    }

    #[test]
    fn test_render_width_bare_esc() {
        // ESC not followed by '[' should be counted as zero-width (control char)
        assert_eq!(display_width("\x1bhello"), 5);
    }

    // -- truncate --

    #[test]
    fn test_render_truncate_no_op() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_render_truncate_exact_fit() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_render_truncate_basic() {
        let result = truncate("hello world", 6);
        assert_eq!(display_width(&result), 6);
        assert!(result.starts_with("hello"));
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn test_render_truncate_to_one() {
        let result = truncate("hello", 1);
        assert_eq!(result, "\u{2026}");
        assert_eq!(display_width(&result), 1);
    }

    #[test]
    fn test_render_truncate_to_zero() {
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn test_render_truncate_cjk() {
        // "日本語" is 6 columns. Truncate to 4: "日…" would be 3, need to check
        let result = truncate("日本語", 4);
        assert!(display_width(&result) <= 4);
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn test_render_truncate_preserves_ansi() {
        let styled = "\x1b[31mhello world\x1b[0m";
        let result = truncate(styled, 6);
        assert_eq!(display_width(&result), 6);
        assert!(result.contains("\x1b[31m"), "ANSI code should be preserved");
        assert!(
            result.contains("%{\x1b[0m%}"),
            "should have ANSI reset wrapped in zsh escapes"
        );
    }

    #[test]
    fn test_render_truncate_preserves_zsh_escape() {
        let styled = "%{\x1b[31m%}hello world%{\x1b[0m%}";
        let result = truncate(styled, 6);
        assert_eq!(display_width(&result), 6);
        assert!(result.contains("%{\x1b[31m%}"));
    }

    // -- double-percent (%%) --

    #[test]
    fn test_render_width_double_percent() {
        // %% displays as a single '%' in zsh PROMPT_PERCENT
        assert_eq!(display_width("%%"), 1);
        assert_eq!(display_width("100%%"), 4);
    }

    #[test]
    fn test_render_width_multiple_double_percent() {
        // '5'(1) + '0'(1) + '%%'(1) = 3
        assert_eq!(display_width("50%%"), 3);
    }

    #[test]
    fn test_render_truncate_double_percent_fits() {
        assert_eq!(truncate("100%%", 4), "100%%"); // display width 4 == 4
        assert_eq!(truncate("100%%", 5), "100%%");
    }

    #[test]
    fn test_render_truncate_double_percent_truncated() {
        let result = truncate("100%%", 3);
        assert_eq!(display_width(&result), 3);
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn test_render_truncate_double_percent_pair_not_split() {
        let result = truncate("XY%%", 2);
        assert_eq!(display_width(&result), 2);
    }

    #[test]
    fn test_render_truncate_double_percent_preserved() {
        // When %% survives truncation it stays as %%
        assert_eq!(truncate("A%%B", 3), "A%%B"); // display width 3 == 3
        assert_eq!(display_width("A%%B"), 3);
    }
}
