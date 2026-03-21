//! Structured prompt segment for Starship-compatible composition.
//!
//! A `Segment` wraps a module's content with optional connector word,
//! icon, and style. The composition layer builds segments from module
//! outputs and renders them left-to-right.

use super::style::{ColorMap, Style};

/// A connector word displayed before a segment (e.g., "on", "via", "at").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Connector {
    /// The connector text (e.g., "on", "via", "at").
    pub(crate) word: String,
    /// Style applied to the connector word.
    pub(crate) style: Style,
}

/// An icon glyph displayed before the segment content (e.g., "", "").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Icon {
    /// The icon glyph (Nerd Font).
    pub(crate) glyph: String,
    /// Style applied to the icon.
    pub(crate) style: Style,
}

/// A composed prompt segment with optional connector, icon, and style.
///
/// Rendering produces: `{connector} {icon} {content}` where each part
/// is independently styled and absent parts are omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    /// The text content of this segment.
    pub(crate) content: String,
    /// Optional connector word before this segment.
    pub(crate) connector: Option<Connector>,
    /// Optional icon before the content.
    pub(crate) icon: Option<Icon>,
    /// Style applied to `content`. `None` means the content is already
    /// styled (e.g., git module output with per-field colors).
    pub(crate) content_style: Option<Style>,
}

impl Segment {
    /// Render this segment into a styled string.
    ///
    /// Parts are separated by single spaces and absent parts are omitted.
    #[must_use]
    pub(crate) fn render(&self, color_map: ColorMap) -> String {
        let mut out = String::with_capacity(self.content.len() + 32);

        if let Some(ref conn) = self.connector {
            out.push_str(&conn.style.paint_with(&conn.word, color_map));
            out.push(' ');
        }

        if let Some(ref icon) = self.icon {
            out.push_str(&icon.style.paint_with(&icon.glyph, color_map));
            out.push(' ');
        }

        if let Some(ref style) = self.content_style {
            out.push_str(&style.paint_with(&self.content, color_map));
        } else {
            out.push_str(&self.content);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        render::{
            layout::display_width,
            style::{Color, ColorMap},
        },
        test_utils::contains_style_sequence,
    };

    fn default_color_map() -> ColorMap {
        ColorMap::default()
    }

    #[test]
    fn test_segment_content_only() {
        let seg = Segment {
            content: "hello".to_owned(),
            connector: None,
            icon: None,
            content_style: None,
        };
        assert_eq!(seg.render(default_color_map()), "hello");
    }

    #[test]
    fn test_segment_with_content_style() {
        let seg = Segment {
            content: "dir".to_owned(),
            connector: None,
            icon: None,
            content_style: Some(Style::new().fg(Color::Cyan).bold()),
        };
        let rendered = seg.render(default_color_map());
        assert!(rendered.contains("dir"), "should contain content");
        assert!(
            contains_style_sequence(&rendered, &[1, 36]),
            "should contain bold cyan: {rendered}"
        );
    }

    #[test]
    fn test_segment_with_connector() {
        let seg = Segment {
            content: "main".to_owned(),
            connector: Some(Connector {
                word: "on".to_owned(),
                style: Style::new().dimmed(),
            }),
            icon: None,
            content_style: None,
        };
        let rendered = seg.render(default_color_map());
        assert!(rendered.contains("on"), "should contain connector");
        assert!(rendered.contains("main"), "should contain content");
        // Connector styled dimmed, then space, then content
        assert!(
            rendered.contains("\x1b[2m"),
            "connector should be dimmed: {rendered}"
        );
    }

    #[test]
    fn test_segment_with_icon() {
        let seg = Segment {
            content: "main".to_owned(),
            connector: None,
            icon: Some(Icon {
                glyph: String::new(),
                style: Style::new().fg(Color::Magenta),
            }),
            content_style: None,
        };
        let rendered = seg.render(default_color_map());
        assert!(rendered.contains(""), "should contain icon");
        assert!(rendered.contains("main"), "should contain content");
    }

    #[test]
    fn test_segment_full() {
        let seg = Segment {
            content: "main".to_owned(),
            connector: Some(Connector {
                word: "on".to_owned(),
                style: Style::new().dimmed(),
            }),
            icon: Some(Icon {
                glyph: String::new(),
                style: Style::new().fg(Color::Magenta),
            }),
            content_style: Some(Style::new().fg(Color::Magenta).bold()),
        };
        let rendered = seg.render(default_color_map());
        assert!(rendered.contains("on"), "should contain connector");
        assert!(rendered.contains(""), "should contain icon");
        assert!(rendered.contains("main"), "should contain content");
    }

    #[test]
    fn test_segment_display_width_excludes_escapes() {
        let seg = Segment {
            content: "dir".to_owned(),
            connector: Some(Connector {
                word: "on".to_owned(),
                style: Style::new().fg(Color::BrightBlack),
            }),
            icon: Some(Icon {
                glyph: "*".to_owned(),
                style: Style::new().fg(Color::Magenta),
            }),
            content_style: Some(Style::new().fg(Color::Cyan)),
        };
        let rendered = seg.render(default_color_map());
        // "on" (2) + " " (1) + "*" (1) + " " (1) + "dir" (3) = 8
        assert_eq!(
            display_width(&rendered),
            8,
            "display width should exclude ANSI escapes: {rendered}"
        );
    }

    #[test]
    fn test_segment_pre_styled_content() {
        let pre_styled = Style::new().fg(Color::Magenta).bold().paint("main");
        let seg = Segment {
            content: pre_styled.clone(),
            connector: None,
            icon: None,
            content_style: None, // pre-styled, no additional style
        };
        assert_eq!(seg.render(default_color_map()), pre_styled);
    }

    #[test]
    fn test_segment_with_bright_black_connector() {
        let seg = Segment {
            content: "main".to_owned(),
            connector: Some(Connector {
                word: "on".to_owned(),
                style: Style::new().fg(Color::BrightBlack),
            }),
            icon: None,
            content_style: None,
        };
        let rendered = seg.render(default_color_map());
        assert!(
            rendered.contains("\x1b[90m"),
            "connector should use bright black ANSI code: {rendered}"
        );
    }

    #[test]
    fn test_segment_uses_custom_color_map() {
        let seg = Segment {
            content: "dir".to_owned(),
            connector: Some(Connector {
                word: "on".to_owned(),
                style: Style::new().fg(Color::BrightBlack),
            }),
            icon: None,
            content_style: Some(Style::new().fg(Color::Blue)),
        };
        let rendered = seg.render(ColorMap {
            blue: 94,
            bright_black: 37,
            ..ColorMap::default()
        });
        assert!(
            rendered.contains("\x1b[37m"),
            "connector remap missing: {rendered}"
        );
        assert!(
            rendered.contains("\x1b[94m"),
            "content remap missing: {rendered}"
        );
    }
}
