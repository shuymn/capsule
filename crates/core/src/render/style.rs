//! ANSI styling with zsh prompt escape wrapping.
//!
//! Produces strings with ANSI color codes wrapped in zsh `%{..%}` escapes
//! so that zsh correctly calculates cursor position.

use anstyle::{AnsiColor, Color as AnstyleColor, Effects, Style as AnstyleStyle};

/// Terminal foreground colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Bright black / gray (ANSI 90).
    BrightBlack,
}

/// Foreground ANSI SGR codes for the supported symbolic colors.
///
/// Only classic/bright foreground codes are accepted. This keeps `color_map`
/// aligned with the existing symbolic color vocabulary and preserves current
/// defaults without introducing 256-color semantics.
#[derive(Debug, Clone, Copy, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ColorMap {
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub red: u8,
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub green: u8,
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub yellow: u8,
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub blue: u8,
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub magenta: u8,
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub cyan: u8,
    #[serde(deserialize_with = "deserialize_foreground_code")]
    pub bright_black: u8,
}

impl Default for ColorMap {
    fn default() -> Self {
        Self {
            red: 31,
            green: 32,
            yellow: 33,
            blue: 34,
            magenta: 35,
            cyan: 36,
            bright_black: 90,
        }
    }
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
        self.paint_with(text, ColorMap::default())
    }

    /// Apply ANSI styling using a caller-provided symbolic color mapping.
    #[must_use]
    pub fn paint_with(&self, text: &str, color_map: ColorMap) -> String {
        use std::fmt::Write;

        if self.fg.is_none() && !self.bold && !self.dimmed {
            return text.to_owned();
        }

        let mut result = String::with_capacity(text.len() + 24);
        let style = self.to_anstyle(color_map);
        result.push_str("%{");
        let _ = write!(result, "{}", style.render());
        result.push_str("%}");
        result.push_str(text);
        result.push_str("%{");
        let _ = write!(result, "{}", style.render_reset());
        result.push_str("%}");
        result
    }

    fn to_anstyle(self, color_map: ColorMap) -> AnstyleStyle {
        let mut style = AnstyleStyle::new();
        if let Some(color) = self.fg {
            style = style.fg_color(Some(color_map.anstyle_color(color)));
        }
        let mut effects = Effects::new();
        if self.bold {
            effects |= Effects::BOLD;
        }
        if self.dimmed {
            effects |= Effects::DIMMED;
        }
        style.effects(effects)
    }
}

impl ColorMap {
    const fn fg_code(self, color: Color) -> u8 {
        match color {
            Color::Red => self.red,
            Color::Green => self.green,
            Color::Yellow => self.yellow,
            Color::Blue => self.blue,
            Color::Magenta => self.magenta,
            Color::Cyan => self.cyan,
            Color::BrightBlack => self.bright_black,
        }
    }

    fn anstyle_color(self, color: Color) -> AnstyleColor {
        AnstyleColor::Ansi(self.ansi_color(color))
    }

    fn ansi_color(self, color: Color) -> AnsiColor {
        match self.fg_code(color) {
            30 => AnsiColor::Black,
            31 => AnsiColor::Red,
            32 => AnsiColor::Green,
            33 => AnsiColor::Yellow,
            34 => AnsiColor::Blue,
            35 => AnsiColor::Magenta,
            36 => AnsiColor::Cyan,
            37 => AnsiColor::White,
            90 => AnsiColor::BrightBlack,
            91 => AnsiColor::BrightRed,
            92 => AnsiColor::BrightGreen,
            93 => AnsiColor::BrightYellow,
            94 => AnsiColor::BrightBlue,
            95 => AnsiColor::BrightMagenta,
            96 => AnsiColor::BrightCyan,
            97 => AnsiColor::BrightWhite,
            _ => unreachable!("color map validated at deserialization time"),
        }
    }
}

fn deserialize_foreground_code<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let code = <u16 as serde::Deserialize>::deserialize(deserializer)?;
    if is_valid_foreground_code(code) {
        u8::try_from(code).map_err(|error| {
            serde::de::Error::custom(format!("invalid ANSI foreground code `{code}`: {error}"))
        })
    } else {
        Err(serde::de::Error::custom(format!(
            "invalid ANSI foreground code `{code}`, expected one of 30..=37 or 90..=97"
        )))
    }
}

const fn is_valid_foreground_code(code: u16) -> bool {
    (code >= 30 && code <= 37) || (code >= 90 && code <= 97)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{render::layout::display_width, test_utils::contains_style_sequence};

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
            contains_style_sequence(&painted, &[1, 32]),
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
            contains_style_sequence(&painted, &[2, 36]),
            "should contain dimmed+cyan ANSI code: {painted}"
        );
    }

    #[test]
    fn test_render_style_bright_black() {
        let style = Style::new().fg(Color::BrightBlack);
        let painted = style.paint("hello");
        assert!(
            painted.contains("\x1b[90m"),
            "should contain bright black ANSI code: {painted}"
        );
    }

    #[test]
    fn test_render_style_uses_custom_color_map() {
        let style = Style::new().fg(Color::Blue).bold();
        let color_map = ColorMap {
            blue: 94,
            ..ColorMap::default()
        };
        let painted = style.paint_with("hello", color_map);
        assert!(
            contains_style_sequence(&painted, &[1, 94]),
            "should contain remapped blue ANSI code: {painted}"
        );
    }
}
