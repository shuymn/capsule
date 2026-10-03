use super::{ConfigError, FormatPart, MAX_FORMAT_BYTES, MAX_FORMAT_DEPTH, Template, ValuePlan};

pub(super) fn compile(
    source: &str,
    values: &[ValuePlan],
    module: &str,
) -> Result<Template, ConfigError> {
    if source.len() > MAX_FORMAT_BYTES {
        return Err(ConfigError::invalid(module, "format exceeds byte limit"));
    }
    let mut remaining = source;
    let parts = parse(&mut remaining, values, module, 0)?;
    Ok(Template(parts))
}

fn parse(
    remaining: &mut &str,
    values: &[ValuePlan],
    module: &str,
    depth: usize,
) -> Result<Vec<FormatPart>, ConfigError> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    while let Some(character) = remaining.chars().next() {
        *remaining = &remaining[character.len_utf8()..];
        match character {
            '{' | '[' if remaining.starts_with(character) => {
                literal.push(character);
                *remaining = &remaining[1..];
            }
            '{' => {
                flush_literal(&mut parts, &mut literal);
                let Some(end) = remaining.find('}') else {
                    return Err(ConfigError::invalid(module, "unclosed format variable"));
                };
                let name = &remaining[..end];
                let Some(index) = values.iter().position(|value| value.name == name) else {
                    return Err(ConfigError::invalid(module, "undefined format variable"));
                };
                parts.push(FormatPart::Value(index));
                *remaining = &remaining[end + 1..];
            }
            '[' => {
                if depth >= MAX_FORMAT_DEPTH {
                    return Err(ConfigError::invalid(module, "format nesting exceeds limit"));
                }
                flush_literal(&mut parts, &mut literal);
                parts.push(FormatPart::Optional(parse(
                    remaining,
                    values,
                    module,
                    depth + 1,
                )?));
            }
            ']' if depth != 0 => {
                flush_literal(&mut parts, &mut literal);
                return Ok(parts);
            }
            _ => literal.push(character),
        }
    }
    if depth != 0 {
        return Err(ConfigError::invalid(
            module,
            "unclosed optional format section",
        ));
    }
    flush_literal(&mut parts, &mut literal);
    Ok(parts)
}

fn flush_literal(parts: &mut Vec<FormatPart>, literal: &mut String) {
    if !literal.is_empty() {
        parts.push(FormatPart::Literal(std::mem::take(literal)));
    }
}
