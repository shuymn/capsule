//! Rendering pipeline for composing prompt lines from module outputs.
//!
//! The pipeline has three stages:
//! - **Style** ([`style`]): ANSI color codes wrapped in zsh `%{..%}` escapes
//! - **Layout** ([`layout`]): display width calculation and truncation
//! - **Composition**: arranging segments into left-aligned prompt lines

pub mod layout;
pub mod segment;
pub mod style;

pub use layout::{display_width, truncate};
pub(crate) use segment::Segment;
pub use style::{Color, Style};

/// Composed prompt output ready for the wire protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptLines {
    /// Info line (line 1 of the prompt).
    pub left1: String,
    /// Input line (line 2 of the prompt).
    pub left2: String,
}

/// Compose [`Segment`]s into prompt lines with left-aligned layout.
///
/// All segments are rendered and joined left-to-right with spaces.
/// When total width exceeds `cols`, the first segment on line 1
/// (directory) is truncated. If still too wide, rightmost segments
/// are dropped one at a time.
#[must_use]
pub(crate) fn compose_segments(line1: &[Segment], line2: &[Segment], cols: usize) -> PromptLines {
    PromptLines {
        left1: compose_line(line1, cols),
        left2: compose_line(line2, cols),
    }
}

fn render_segments(segments: &[Segment]) -> Vec<String> {
    segments.iter().map(Segment::render).collect()
}

fn join_rendered(parts: &[String]) -> String {
    let mut out = String::new();
    for part in parts.iter().filter(|s| !s.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

fn compose_line(segments: &[Segment], cols: usize) -> String {
    if cols == 0 || segments.is_empty() {
        return String::new();
    }

    let rendered = render_segments(segments);
    let joined = join_rendered(&rendered);
    let width = display_width(&joined);

    if width <= cols {
        return joined;
    }

    // Try truncating the first rendered segment (directory)
    if let Some(line) = try_truncate_first_segment(&rendered, cols) {
        return line;
    }

    // Drop segments from the end, one at a time
    for drop_count in 1..rendered.len() {
        let remaining = &rendered[..rendered.len() - drop_count];
        let rj = join_rendered(remaining);

        if display_width(&rj) <= cols {
            return rj;
        }

        if let Some(line) = try_truncate_first_segment(remaining, cols) {
            return line;
        }
    }

    // Last resort: truncate everything to cols
    truncate(&joined, cols)
}

fn try_truncate_first_segment(rendered: &[String], cols: usize) -> Option<String> {
    if rendered.is_empty() {
        return None;
    }

    let rest_joined = join_rendered(&rendered[1..]);
    let rest_w = display_width(&rest_joined);

    let sep = usize::from(!rest_joined.is_empty());
    let overhead = rest_w + sep;

    if overhead >= cols {
        return None;
    }

    let available = cols - overhead;
    let dir_truncated = truncate(&rendered[0], available);

    let mut out = dir_truncated;
    if !rest_joined.is_empty() {
        out.push(' ');
        out.push_str(&rest_joined);
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{segment::Connector, *};

    fn seg(content: &str) -> Segment {
        Segment {
            content: content.to_owned(),
            connector: None,
            icon: None,
            content_style: None,
        }
    }

    fn seg_with_connector(content: &str, word: &str) -> Segment {
        Segment {
            content: content.to_owned(),
            connector: Some(Connector {
                word: word.to_owned(),
                style: Style::new().dimmed(),
            }),
            icon: None,
            content_style: None,
        }
    }

    #[test]
    fn test_compose_segments_basic() {
        let result = compose_segments(
            &[seg("dir"), seg_with_connector("main", "on")],
            &[seg("❯")],
            80,
        );
        assert!(
            result.left1.contains("dir"),
            "should contain dir: {}",
            result.left1
        );
        assert!(
            result.left1.contains("on"),
            "should contain connector: {}",
            result.left1
        );
        assert!(
            result.left1.contains("main"),
            "should contain branch: {}",
            result.left1
        );
        assert_eq!(result.left2, "❯");
    }

    #[test]
    fn test_compose_segments_left_aligned() {
        let result = compose_segments(&[seg("dir"), seg("rust")], &[seg("❯")], 80);
        // No right-padding: display width should be less than cols
        assert!(
            display_width(&result.left1) < 80,
            "should not right-pad: width={}, line={}",
            display_width(&result.left1),
            result.left1
        );
        assert!(result.left1.starts_with("dir"), "left1: {}", result.left1);
    }

    #[test]
    fn test_compose_segments_truncates_first() {
        let result = compose_segments(
            &[seg("very/long/directory/path/here"), seg("git")],
            &[seg("❯")],
            15,
        );
        assert!(
            display_width(&result.left1) <= 15,
            "should fit in cols: width={}, line={}",
            display_width(&result.left1),
            result.left1
        );
        assert!(
            result.left1.contains("git"),
            "should preserve later segments: {}",
            result.left1
        );
    }

    #[test]
    fn test_compose_segments_drops_rightmost() {
        let result = compose_segments(
            &[seg("dir"), seg("segment-aaa"), seg("segment-bbb")],
            &[seg("❯")],
            20,
        );
        assert!(
            display_width(&result.left1) <= 20,
            "should fit: width={}, line={}",
            display_width(&result.left1),
            result.left1
        );
        // dir (3) + " " (1) + segment-aaa (11) = 15 fits, segment-bbb dropped
        assert!(
            result.left1.contains("dir"),
            "directory preserved: {}",
            result.left1
        );
        assert!(
            !result.left1.contains("segment-bbb"),
            "rightmost should be dropped: {}",
            result.left1
        );
    }

    #[test]
    fn test_compose_segments_zero_cols() {
        let result = compose_segments(&[seg("hello")], &[seg("❯")], 0);
        assert_eq!(result.left1, "");
        assert_eq!(result.left2, "");
    }

    #[test]
    fn test_compose_segments_empty() {
        let result = compose_segments(&[], &[], 80);
        assert_eq!(result.left1, "");
        assert_eq!(result.left2, "");
    }

    #[test]
    fn test_compose_segments_with_styled_content() {
        let styled_seg = Segment {
            content: "project".to_owned(),
            connector: None,
            icon: None,
            content_style: Some(Style::new().fg(Color::Cyan)),
        };
        let result = compose_segments(&[styled_seg], &[seg("❯")], 30);
        assert!(
            result.left1.contains("project"),
            "should contain content: {}",
            result.left1
        );
        assert!(
            result.left1.contains("\x1b[36m"),
            "should contain cyan ANSI: {}",
            result.left1
        );
    }
}
