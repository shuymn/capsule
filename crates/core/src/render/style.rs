//! ANSI styling with zsh prompt escape wrapping.
//!
//! Produces strings with ANSI color codes wrapped in zsh `%{..%}` escapes
//! so that zsh correctly calculates cursor position.

use std::fmt::Write;

/// Terminal foreground colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Red (ANSI 31).
    Red,
    /// Green (ANSI 32).
    Green,
    /// Yellow (ANSI 33).
    Yellow,
    /// Blue (ANSI 34).
    Blue,
    /// Magenta (ANSI 35).
    Magenta,
    /// Cyan (ANSI 36).
    Cyan,
}

/// A text style with optional foreground color, bold, and dimmed attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    fg: Option<Color>,
    bold: bool,
    dimmed: bool,
}

impl Style {
    /// Creates a new unstyled `Style`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bold: false,
            dimmed: false,
        }
    }

    /// Sets the foreground color.
    #[must_use]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Enables bold.
    #[must_use]
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Enables dimmed (faint) rendering.
    #[must_use]
    pub const fn dimmed(mut self) -> Self {
        self.dimmed = true;
        self
    }

    /// Apply ANSI styling to `text`, wrapping escape sequences in zsh `%{..%}`.
    ///
    /// Returns `text` unchanged when no style attributes are set.
    #[must_use]
    pub fn paint(&self, text: &str) -> String {
        if self.fg.is_none() && !self.bold && !self.dimmed {
            return text.to_owned();
        }

        let mut codes = String::with_capacity(8);
        if self.bold {
            codes.push('1');
        }
        if self.dimmed {
            if !codes.is_empty() {
                codes.push(';');
            }
            codes.push('2');
        }
        if let Some(color) = self.fg {
            if !codes.is_empty() {
                codes.push(';');
            }
            let _ = write!(codes, "{}", color.fg_code());
        }

        let mut result = String::with_capacity(text.len() + 24);
        let _ = write!(result, "%{{\x1b[{codes}m%}}{text}%{{\x1b[0m%}}");
        result
    }
}

impl Color {
    const fn fg_code(self) -> u8 {
        match self {
            Self::Red => 31,
            Self::Green => 32,
            Self::Yellow => 33,
            Self::Blue => 34,
            Self::Magenta => 35,
            Self::Cyan => 36,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::layout::display_width;

    #[test]
    fn test_render_style_no_style() {
        let style = Style::new();
        assert_eq!(style.paint("hello"), "hello");
    }

    #[test]
    fn test_render_style_foreground_color() {
        let style = Style::new().fg(Color::Red);
        let painted = style.paint("hello");
        assert!(painted.contains("\x1b[31m"), "should contain red ANSI code");
        assert!(painted.contains("\x1b[0m"), "should contain reset");
        assert!(painted.contains("%{"), "should have zsh escape open");
        assert!(painted.contains("%}"), "should have zsh escape close");
        assert!(painted.contains("hello"), "text should be present");
    }

    #[test]
    fn test_render_style_bold() {
        let style = Style::new().bold();
        let painted = style.paint("hello");
        assert!(painted.contains("\x1b[1m"), "should contain bold ANSI code");
    }

    #[test]
    fn test_render_style_bold_and_color() {
        let style = Style::new().fg(Color::Green).bold();
        let painted = style.paint("hello");
        assert!(
            painted.contains("\x1b[1;32m"),
            "should contain bold+green ANSI code"
        );
    }

    #[test]
    fn test_render_style_display_width_unchanged() {
        let style = Style::new().fg(Color::Cyan).bold();
        let painted = style.paint("hello");
        assert_eq!(
            display_width(&painted),
            5,
            "styled text should have same display width as plain text"
        );
    }

    #[test]
    fn test_render_style_default_is_unstyled() {
        let style = Style::default();
        assert_eq!(style.paint("test"), "test");
    }

    #[test]
    fn test_render_style_dimmed() {
        let style = Style::new().dimmed();
        let painted = style.paint("hello");
        assert!(
            painted.contains("\x1b[2m"),
            "should contain dimmed ANSI code"
        );
        assert!(painted.contains("\x1b[0m"), "should contain reset");
        assert!(painted.contains("hello"), "text should be present");
    }

    #[test]
    fn test_render_style_dimmed_and_color() {
        let style = Style::new().fg(Color::Cyan).dimmed();
        let painted = style.paint("hello");
        assert!(
            painted.contains("\x1b[2;36m"),
            "should contain dimmed+cyan ANSI code: {painted}"
        );
    }

    #[test]
    fn test_render_style_dimmed_display_width_unchanged() {
        let style = Style::new().dimmed();
        let painted = style.paint("hello");
        assert_eq!(
            display_width(&painted),
            5,
            "dimmed text should have same display width as plain text"
        );
    }
}
