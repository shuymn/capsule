use std::fmt::Write;

/// Checks whether `text` contains ANSI SGR codes matching the given sequence.
///
/// Different terminal libraries may emit combined (`\x1b[1;32m`) or split
/// (`\x1b[1m\x1b[32m`) sequences. This helper accepts either form.
pub fn contains_style_sequence(text: &str, codes: &[u8]) -> bool {
    let combined = format!(
        "\x1b[{}m",
        codes
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(";")
    );
    let mut split = String::with_capacity(codes.len() * 5);
    for code in codes {
        let _ = write!(split, "\x1b[{code}m");
    }
    text.contains(&combined) || text.contains(&split)
}
