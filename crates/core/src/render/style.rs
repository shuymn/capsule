//! ANSI styling with zsh prompt escape wrapping.
//!
//! Produces strings with ANSI color codes wrapped in zsh `%{..%}` escapes
//! so that zsh correctly calculates cursor position.

use anstyle::{AnsiColor, Color as AnstyleColor, Effects, Style as AnstyleStyle};

/// Terminal foreground colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
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
#[serde(default, deny_unknown_fields)]
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
    /// `text` is quoted as in [`Style::paint_with`].
    #[must_use]
    pub fn paint(&self, text: &str) -> String {
        self.paint_with(text, ColorMap::default())
    }

    /// Apply ANSI styling using a caller-provided symbolic color mapping.
    ///
    /// `text` displays literally whether zsh expands the prompt with
    /// `PROMPT_SUBST` on or off. `%` becomes `%%`; `$`, `` ` ``, and `\` are
    /// backslash-escaped inside a one-column truncation, `%1<<\$%<<`, which
    /// hides the backslash when prompt substitution has not removed it.
    #[must_use]
    pub fn paint_with(&self, text: &str, color_map: ColorMap) -> String {
        use std::fmt::Write;

        let escaped = quote_prompt_text(text);

        if self.fg.is_none() && !self.bold && !self.dimmed {
            return escaped;
        }

        let mut result = String::with_capacity(escaped.len() + 24);
        let style = self.to_anstyle(color_map);
        result.push_str("%{");
        let _ = write!(result, "{}", style.render());
        result.push_str("%}");
        result.push_str(&escaped);
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
    u8::try_from(code)
        .ok()
        .filter(|code| matches!(code, 30..=37 | 90..=97))
        .ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid ANSI foreground code `{code}`, expected one of 30..=37 or 90..=97"
            ))
        })
}

fn quote_prompt_text(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '%' => quoted.push_str("%%"),
            '$' | '`' | '\\' => {
                quoted.push_str("%1<<\\");
                quoted.push(character);
                quoted.push_str("%<<");
            }
            character => quoted.push(character),
        }
    }
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{contains_style_sequence, styled};

    #[test]
    fn paint_wraps_styled_text_in_zsh_escapes_and_quotes_percent() {
        assert_eq!(Style::new().paint("100%B"), "100%%B");
        for (style, codes) in [
            (Style::new().fg(Color::Red), &[31][..]),
            (Style::new().bold(), &[1]),
            (Style::new().fg(Color::Green).bold(), &[1, 32]),
            (Style::new().dimmed(), &[2]),
            (Style::new().fg(Color::Cyan).dimmed(), &[2, 36]),
            (Style::new().fg(Color::BrightBlack), &[90]),
        ] {
            assert_eq!(style.paint("100%B"), styled(codes, "100%%B"));
        }
    }

    #[test]
    fn color_map_accepts_exactly_the_classic_and_bright_foreground_codes() {
        let style = Style::new().fg(Color::Blue).bold();
        for code in (30..=37).chain(90..=97) {
            let painted = toml::from_str::<ColorMap>(&format!("blue = {code}"))
                .map(|color_map| style.paint_with("hello", color_map));
            assert!(
                painted.is_ok_and(|painted| contains_style_sequence(&painted, &[1, code])),
                "code {code} not painted"
            );
        }
        for code in [29, 38, 89, 98, 256] {
            assert!(
                toml::from_str::<ColorMap>(&format!("blue = {code}")).is_err(),
                "accepted {code}"
            );
        }
    }
}
