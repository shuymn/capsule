use std::{collections::HashMap, path::Path};

use regex_lite::Regex;

use super::{
    super::ModuleSpeed, CustomModuleInfo, DetectedModuleCandidate, RequestFacts, ResolvedModule,
};

/// Detects all matching custom modules for the given directory.
///
/// `env_vars` provides environment variable values forwarded from the shell.
/// `path_env` overrides PATH for command execution (launchd support).
///
/// Fast modules only try env/file sources. Slow modules try env/file first,
/// then command sources on failure.
#[must_use]
pub fn detect_modules(
    defs: &[ResolvedModule],
    cwd: &Path,
    env_vars: &[(String, String)],
    path_env: Option<&str>,
    only_speed: ModuleSpeed,
) -> Vec<CustomModuleInfo> {
    let facts = RequestFacts::collect(cwd.to_path_buf(), env_vars.to_vec())
        .with_command_path_env(path_env.map(ToOwned::to_owned));
    let detected = facts
        .matching_modules(defs, only_speed)
        .into_iter()
        .filter_map(|(_, module)| {
            facts
                .detect_module(module)
                .map(|info| DetectedModuleCandidate::new(module, info))
        })
        .collect();
    arbitrate_detected_modules(detected)
}

/// Collapse competing detected modules while preserving definition order.
#[must_use]
pub fn arbitrate_detected_modules(detected: Vec<DetectedModuleCandidate>) -> Vec<CustomModuleInfo> {
    let mut winners = HashMap::<String, (usize, u32)>::new();

    for (idx, candidate) in detected.iter().enumerate() {
        let Some(arbitration) = &candidate.arbitration else {
            continue;
        };
        winners
            .entry(arbitration.group.clone())
            .and_modify(|winner| {
                if arbitration.priority < winner.1 {
                    *winner = (idx, arbitration.priority);
                }
            })
            .or_insert((idx, arbitration.priority));
    }

    detected
        .into_iter()
        .enumerate()
        .filter_map(|(idx, candidate)| match &candidate.arbitration {
            None => Some(candidate.info),
            Some(arbitration) => winners
                .get(&arbitration.group)
                .is_some_and(|winner| winner.0 == idx)
                .then_some(candidate.info),
        })
        .collect()
}

pub(super) fn apply_regex(input: &str, regex: Option<&Regex>) -> Option<String> {
    if let Some(regex) = regex {
        let captures = regex.captures(input)?;
        Some(captures.get(1)?.as_str().to_owned())
    } else {
        Some(input.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Format string parser and renderer
// ---------------------------------------------------------------------------

/// A parsed segment of a module format string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatSegment {
    /// Literal text.
    Literal(String),
    /// A required variable reference: `{name}`.
    Variable(String),
    /// An optional section: `[content]`.
    /// Omitted entirely if any contained variable is unresolved.
    Optional(Vec<Self>),
}

/// Parses a format string into [`FormatSegment`]s.
///
/// Syntax:
/// - `{name}` — variable placeholder
/// - `[content with {name}]` — optional section (omitted if any variable unresolved)
/// - `{{` — escaped literal `{`
/// - `[[` — escaped literal `[`
pub fn parse_format(input: &str) -> Vec<FormatSegment> {
    let mut segments = Vec::new();
    parse_segments(input, &mut segments, false);
    segments
}

/// Recursive parser. Returns the number of bytes consumed.
fn parse_segments(input: &str, segments: &mut Vec<FormatSegment>, in_optional: bool) -> usize {
    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut literal = String::new();

    while pos < bytes.len() {
        match bytes[pos] {
            b'{' if pos + 1 < bytes.len() && bytes[pos + 1] == b'{' => {
                literal.push('{');
                pos += 2;
            }
            b'{' => {
                if !literal.is_empty() {
                    segments.push(FormatSegment::Literal(std::mem::take(&mut literal)));
                }
                pos += 1; // skip '{'
                let start = pos;
                while pos < bytes.len() && bytes[pos] != b'}' {
                    pos += 1;
                }
                let name = &input[start..pos];
                segments.push(FormatSegment::Variable(name.to_owned()));
                if pos < bytes.len() {
                    pos += 1; // skip '}'
                }
            }
            b'[' if pos + 1 < bytes.len() && bytes[pos + 1] == b'[' => {
                literal.push('[');
                pos += 2;
            }
            b'[' => {
                if !literal.is_empty() {
                    segments.push(FormatSegment::Literal(std::mem::take(&mut literal)));
                }
                pos += 1; // skip '['
                let mut inner = Vec::new();
                let consumed = parse_segments(&input[pos..], &mut inner, true);
                pos += consumed;
                segments.push(FormatSegment::Optional(inner));
            }
            b']' if in_optional => {
                pos += 1; // skip ']'
                break;
            }
            _ => {
                // Handle multi-byte UTF-8: advance by character, not byte.
                let ch = input[pos..].chars().next().unwrap_or('\0');
                literal.push(ch);
                pos += ch.len_utf8();
            }
        }
    }

    if !literal.is_empty() {
        segments.push(FormatSegment::Literal(literal));
    }

    pos
}

/// Renders parsed format segments using resolved variable values.
///
/// Returns `None` if a required variable (outside `[optional]` sections) is
/// unresolved, meaning the module should not be detected.
pub fn render_format(segments: &[FormatSegment], values: &HashMap<&str, String>) -> Option<String> {
    render_segments(segments, values, false)
}

/// Renders segments recursively. In optional context, unresolved variables
/// cause the entire section to be omitted rather than failing the module.
fn render_segments(
    segments: &[FormatSegment],
    values: &HashMap<&str, String>,
    in_optional: bool,
) -> Option<String> {
    let mut output = String::new();
    for segment in segments {
        match segment {
            FormatSegment::Literal(text) => output.push_str(text),
            FormatSegment::Variable(name) => {
                let value = values.get(name.as_str())?;
                output.push_str(value);
            }
            FormatSegment::Optional(inner) => {
                if !in_optional && let Some(rendered) = render_segments(inner, values, true) {
                    output.push_str(&rendered);
                }
            }
        }
    }
    Some(output)
}

/// Formats a detected module using pre-parsed format segments and resolved
/// variable values.
///
/// Returns `None` if any required variable is unresolved.
pub(super) fn format_module(
    def: &ResolvedModule,
    values: &HashMap<&str, String>,
) -> Option<CustomModuleInfo> {
    let value = render_format(&def.format_segments, values)?;
    Some(CustomModuleInfo {
        name: def.name.clone(),
        value,
        icon: def.icon.clone(),
        style: def.style,
        connector: def.connector.clone(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_format ---------------------------------------------------------

    #[test]
    fn test_parse_literal_only() {
        let segments = parse_format("hello world");
        assert_eq!(
            segments,
            vec![FormatSegment::Literal("hello world".to_owned())]
        );
    }

    #[test]
    fn test_parse_single_variable() {
        let segments = parse_format("{version}");
        assert_eq!(
            segments,
            vec![FormatSegment::Variable("version".to_owned())]
        );
    }

    #[test]
    fn test_parse_variable_with_surrounding_text() {
        let segments = parse_format("v{version}!");
        assert_eq!(
            segments,
            vec![
                FormatSegment::Literal("v".to_owned()),
                FormatSegment::Variable("version".to_owned()),
                FormatSegment::Literal("!".to_owned()),
            ]
        );
    }

    #[test]
    fn test_parse_multiple_variables() {
        let segments = parse_format("{profile} ({region})");
        assert_eq!(
            segments,
            vec![
                FormatSegment::Variable("profile".to_owned()),
                FormatSegment::Literal(" (".to_owned()),
                FormatSegment::Variable("region".to_owned()),
                FormatSegment::Literal(")".to_owned()),
            ]
        );
    }

    #[test]
    fn test_parse_optional_section() {
        let segments = parse_format("{profile}[ ({region})]");
        assert_eq!(
            segments,
            vec![
                FormatSegment::Variable("profile".to_owned()),
                FormatSegment::Optional(vec![
                    FormatSegment::Literal(" (".to_owned()),
                    FormatSegment::Variable("region".to_owned()),
                    FormatSegment::Literal(")".to_owned()),
                ]),
            ]
        );
    }

    #[test]
    fn test_parse_escaped_braces() {
        let segments = parse_format("{{literal}}");
        // `{{` → literal `{`, then `}}` → `}` closes nothing (no open `{` for variable) + trailing `}`
        assert_eq!(
            segments,
            vec![FormatSegment::Literal("{literal}}".to_owned())]
        );
    }

    #[test]
    fn test_parse_escaped_brackets() {
        let segments = parse_format("[[literal]]");
        assert_eq!(
            segments,
            vec![FormatSegment::Literal("[literal]]".to_owned())]
        );
    }

    #[test]
    fn test_parse_empty_string() {
        let segments = parse_format("");
        assert!(segments.is_empty());
    }

    // -- render_format --------------------------------------------------------

    #[test]
    fn test_render_all_variables_resolved() {
        let segments = parse_format("{profile} ({region})");
        let mut values = HashMap::new();
        values.insert("profile", "prod".to_owned());
        values.insert("region", "us-east-1".to_owned());
        assert_eq!(
            render_format(&segments, &values),
            Some("prod (us-east-1)".to_owned())
        );
    }

    #[test]
    fn test_render_required_variable_missing_returns_none() {
        let segments = parse_format("{profile} ({region})");
        let mut values = HashMap::new();
        values.insert("profile", "prod".to_owned());
        // region missing
        assert_eq!(render_format(&segments, &values), None);
    }

    #[test]
    fn test_render_optional_section_with_resolved_variable() {
        let segments = parse_format("{profile}[ ({region})]");
        let mut values = HashMap::new();
        values.insert("profile", "prod".to_owned());
        values.insert("region", "us-east-1".to_owned());
        assert_eq!(
            render_format(&segments, &values),
            Some("prod (us-east-1)".to_owned())
        );
    }

    #[test]
    fn test_render_optional_section_with_missing_variable() {
        let segments = parse_format("{profile}[ ({region})]");
        let mut values = HashMap::new();
        values.insert("profile", "prod".to_owned());
        // region missing → optional section omitted
        assert_eq!(render_format(&segments, &values), Some("prod".to_owned()));
    }

    #[test]
    fn test_render_all_optional_missing_required_present() {
        let segments = parse_format("{name}[ v{version}][ @{scope}]");
        let mut values = HashMap::new();
        values.insert("name", "capsule".to_owned());
        assert_eq!(
            render_format(&segments, &values),
            Some("capsule".to_owned())
        );
    }

    #[test]
    fn test_render_literal_only() {
        let segments = parse_format("static text");
        let values = HashMap::new();
        assert_eq!(
            render_format(&segments, &values),
            Some("static text".to_owned())
        );
    }

    #[test]
    fn test_render_no_recursive_expansion() {
        let segments = parse_format("prefix-{value}-suffix");
        let mut values = HashMap::new();
        values.insert("value", "{value}".to_owned());
        assert_eq!(
            render_format(&segments, &values),
            Some("prefix-{value}-suffix".to_owned())
        );
    }
}
