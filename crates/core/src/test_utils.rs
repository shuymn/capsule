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
