//! Unescaped styled text. Width and truncation never inspect serialized ANSI.

use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::render::style::{ColorMap, Style};

// Leave room for styles, percent quoting, wire escaping, and the second line
// within the session protocol's 64 KiB response limit.
const MAX_LINE_TEXT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    text: String,
    style: Style,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StyledText(Vec<Span>);

impl StyledText {
    pub(super) fn new(text: &str, style: Style) -> Self {
        let text = sanitize(text);
        if text.is_empty() {
            Self::default()
        } else {
            Self(vec![Span { text, style }])
        }
    }

    pub(super) fn push(&mut self, text: &str, style: Style) {
        self.append(Self::new(text, style));
    }

    fn append(&mut self, other: Self) {
        for span in other.0 {
            if let Some(last) = self.0.last_mut()
                && last.style == span.style
            {
                last.text.push_str(&span.text);
            } else {
                self.0.push(span);
            }
        }
    }

    pub(super) fn join(parts: &[Self]) -> Self {
        let mut joined = Self::default();
        for part in parts.iter().filter(|part| !part.is_empty()) {
            if !joined.is_empty() {
                joined.push(" ", Style::new());
            }
            joined.append(part.clone());
        }
        joined
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn plain_text(&self) -> String {
        self.0.iter().map(|span| span.text.as_str()).collect()
    }

    pub(super) fn width(&self) -> usize {
        self.plain_text().width()
    }

    pub(super) fn truncate(&self, max_width: usize) -> Self {
        let plain = self.plain_text();
        if plain.width() <= max_width && plain.len() <= MAX_LINE_TEXT_BYTES {
            return self.clone();
        }
        if max_width == 0 {
            return Self::default();
        }
        let mut width = 0;
        let mut end = 0;
        // Segment the complete text, so a style boundary cannot split a cluster.
        for (index, grapheme) in plain.grapheme_indices(true) {
            let next_width = width + grapheme.width();
            if next_width > max_width - 1
                || index + grapheme.len() > MAX_LINE_TEXT_BYTES - "…".len()
            {
                break;
            }
            width = next_width;
            end = index + grapheme.len();
        }
        let mut truncated = Self::default();
        let mut remaining = end;
        for span in &self.0 {
            if remaining == 0 {
                break;
            }
            let take = remaining.min(span.text.len());
            truncated.push(&span.text[..take], span.style);
            remaining -= take;
        }
        let style = truncated
            .0
            .last()
            .or_else(|| self.0.first())
            .map_or(Style::new(), |span| span.style);
        truncated.push("…", style);
        truncated
    }

    pub(super) fn to_zsh(&self, color_map: ColorMap) -> String {
        self.0
            .iter()
            .map(|span| span.style.paint_with(&span.text, color_map))
            .collect()
    }
}

fn sanitize(text: &str) -> String {
    let mut sanitized = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control()
            || matches!(character, '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            sanitized.extend(character.escape_default());
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::style::Color;

    #[test]
    fn width_counts_complete_graphemes_before_escaping() {
        for (text, width) in [
            ("e\u{301}", 1),
            ("👩🏽‍💻", 2),
            ("🇯🇵", 2),
            ("日本語", 6),
            ("%B", 2),
        ] {
            assert_eq!(StyledText::new(text, Style::new()).width(), width, "{text}");
        }
    }

    #[test]
    fn truncation_keeps_combining_and_emoji_clusters_intact() {
        for (text, cols, expected) in [
            ("e\u{301}xy", 2, "e\u{301}…"),
            ("👩🏽‍💻xy", 3, "👩🏽‍💻…"),
            ("👩🏽‍💻xy", 2, "…"),
            ("🇯🇵xy", 3, "🇯🇵…"),
            ("日本語", 4, "日…"),
            ("abc", 0, ""),
            ("abc", 1, "…"),
        ] {
            let result = StyledText::new(text, Style::new()).truncate(cols);
            assert_eq!(result.plain_text(), expected);
            assert!(result.width() <= cols);
        }
    }

    #[test]
    fn a_style_boundary_cannot_split_a_grapheme() {
        let mut text = StyledText::new("👩", Style::new().fg(Color::Red));
        text.push("🏽‍💻xy", Style::new().fg(Color::Blue));
        assert_eq!(text.width(), 4);
        assert_eq!(text.truncate(3).plain_text(), "👩🏽‍💻…");
        assert_eq!(text.truncate(2).plain_text(), "…");
    }

    #[test]
    fn controls_are_display_data_before_width_calculation() {
        let text = StyledText::new("\x1b[31m\n\t\x7f\u{202e}%B$HOME`echo no`", Style::new());
        let plain = text.plain_text();
        assert!(!plain.chars().any(char::is_control));
        assert!(plain.contains("\\n\\t"));
        assert!(plain.contains("\\u{202e}"));
        assert_eq!(text.width(), plain.width());
        let serialized = text.to_zsh(ColorMap::default());
        assert!(serialized.contains("%%B$HOME`echo no`"));
        assert!(!serialized.contains('\x1b'));
    }
}
