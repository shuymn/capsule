use std::fmt::Write;

/// Resolve a test program before replacing its environment with a snapshot.
pub fn executable(name: &str) -> std::io::Result<String> {
    let path = std::env::var_os("PATH")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing PATH"))?;
    let program = std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, name.to_owned()))?;
    let program = std::path::absolute(program)?;
    std::str::from_utf8(program.as_os_str().as_encoded_bytes())
        .map(str::to_owned)
        .map_err(std::io::Error::other)
}

/// Write an executable `sh` script at `path` without this process ever
/// holding a writable descriptor to it.
///
/// A child forked by a parallel test can inherit such a descriptor until its
/// own exec, and executing the script then fails with ETXTBSY. Copying from a
/// non-executable source in a `cp` subprocess keeps the writer out of this
/// process.
pub fn write_script(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let source = path.with_extension("source");
    std::fs::write(&source, format!("#!{}\n{body}\n", executable("sh")?))?;
    let status = std::process::Command::new(executable("cp")?)
        .arg(&source)
        .arg(path)
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("cp failed: {status}")));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

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

/// Characters that stress display width, grapheme segmentation, sanitizing,
/// and prompt quoting: ASCII, CJK, combining marks, ZWJ emoji parts, regional
/// indicators, variation selectors, a lam-alef pair, controls, bidi controls,
/// and zsh prompt metacharacters.
const HOSTILE: &[char] = &[
    'a', 'Z', '0', ' ', '-', '日', '本', '\u{301}', '\u{200d}', '\u{200b}', '👩', '🏽', '💻', '🇯',
    '🇵', '\u{fe0f}', '\u{fe0e}', 'ل', 'ا', '\x1b', '\n', '\t', '\x7f', '\u{85}', '\u{202e}',
    '\u{2066}', '\u{200f}', '%', '$', '`', '\\', '{', '}', '[', ']', '…',
];

/// One character, usually from [`HOSTILE`], sometimes any scalar value.
pub fn hostile_char() -> impl proptest::strategy::Strategy<Value = char> {
    use proptest::prelude::*;

    prop_oneof![9 => proptest::sample::select(HOSTILE), 1 => any::<char>()]
}

/// Up to `max` hostile characters.
pub fn hostile_text(max: usize) -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::prelude::*;

    proptest::collection::vec(hostile_char(), 0..=max).prop_map(String::from_iter)
}

/// Hostile text, sometimes repeated past the per-line and per-module byte bounds.
pub fn bulky_text() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::prelude::*;

    prop_oneof![
        4 => hostile_text(16),
        1 => (hostile_text(4), 0..20_000usize).prop_map(|(text, count)| text.repeat(count)),
    ]
}

/// What zsh displays for a serialized prompt line: `%{…%}` sequences removed,
/// `%%` and Capsule's `%1<<\X%<<` quoting undone. `None` for any other `%`
/// escape, which Capsule never emits for data.
pub fn displayed_prompt(line: &str) -> Option<String> {
    let mut displayed = String::new();
    let mut rest = line;
    while let Some(character) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("%{") {
            rest = &after[after.find("%}")? + 2..];
        } else if let Some(after) = rest.strip_prefix("%%") {
            displayed.push('%');
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%1<<\\") {
            let quoted = after.chars().next()?;
            displayed.push(quoted);
            rest = after[quoted.len_utf8()..].strip_prefix("%<<")?;
        } else if character == '%' {
            return None;
        } else {
            displayed.push(character);
            rest = &rest[character.len_utf8()..];
        }
    }
    Some(displayed)
}
