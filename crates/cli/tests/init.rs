//! Integration tests for `capsule init zsh`.

use std::process::Command;

/// Path to the init.zsh source file for direct zsh function testing.
const INIT_ZSH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/src/init/init.zsh");

/// Helper: run a zsh snippet that sources init.zsh (without calling _capsule_init)
/// and executes the given test code. Returns stdout.
fn run_zsh_snippet(code: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Source only function definitions, skip _capsule_init call at the end.
    // We do this by defining _capsule_init as a no-op before sourcing.
    let script = format!(
        r#"
emulate -L zsh
_capsule_init() {{ : }}
source {INIT_ZSH}
{code}
"#
    );
    let output = Command::new("zsh").args(["-c", &script]).output()?;
    if !output.status.success() {
        return Err(format!(
            "zsh exited with {}: stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn test_init_zsh_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .args(["init", "zsh"])
        .output()?;

    assert!(output.status.success(), "exit status: {}", output.status);
    assert!(
        !output.stdout.is_empty(),
        "init zsh should produce output on stdout"
    );
    Ok(())
}

#[test]
fn test_init_zsh_output_contains_capsule_functions() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .args(["init", "zsh"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("_capsule_precmd"),
        "missing _capsule_precmd"
    );
    assert!(
        stdout.contains("_capsule_preexec"),
        "missing _capsule_preexec"
    );
    assert!(stdout.contains("_capsule_init"), "missing _capsule_init");
    assert!(
        stdout.contains("_capsule_start_coproc"),
        "missing _capsule_start_coproc"
    );
    assert!(
        stdout.contains("_capsule_async_callback"),
        "missing _capsule_async_callback"
    );
    Ok(())
}

/// Executable doc test: eval the init script in an isolated zsh session
/// and verify PROMPT is set to a non-empty value.
#[test]
fn test_init_zsh_sets_prompt_in_isolated_session() -> Result<(), Box<dyn std::error::Error>> {
    let capsule_bin = env!("CARGO_BIN_EXE_capsule");
    let capsule_dir = std::path::Path::new(capsule_bin)
        .parent()
        .ok_or("binary has no parent dir")?;

    let tmp = tempfile::tempdir()?;

    // Add capsule binary dir to PATH so the zsh script can find `capsule connect`
    let path_env = format!(
        "{}:{}",
        capsule_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new("zsh")
        .args([
            "-c",
            &format!(r#"eval "$({capsule_bin} init zsh)" && [[ -n "$PROMPT" ]]"#,),
        ])
        .env("ZDOTDIR", tmp.path())
        .env("HOME", tmp.path())
        .env("TMPDIR", tmp.path())
        .env("PATH", &path_env)
        .output()?;

    assert!(
        output.status.success(),
        "zsh exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

// -- Pure function tests: _capsule_ns (netstring encoding) --------------------

#[test]
fn test_zsh_ns_ascii() -> Result<(), Box<dyn std::error::Error>> {
    let out = run_zsh_snippet(r#"_capsule_ns "hello"; print -r -- "$REPLY""#)?;
    assert_eq!(out.trim(), "5:hello,", "ASCII netstring encoding");
    Ok(())
}

#[test]
fn test_zsh_ns_empty() -> Result<(), Box<dyn std::error::Error>> {
    let out = run_zsh_snippet(r#"_capsule_ns ""; print -r -- "$REPLY""#)?;
    assert_eq!(out.trim(), "0:,", "empty netstring encoding");
    Ok(())
}

#[test]
fn test_zsh_ns_utf8_counts_bytes() -> Result<(), Box<dyn std::error::Error>> {
    // "日本" is 6 bytes in UTF-8 (3 bytes per character)
    let out = run_zsh_snippet(r#"_capsule_ns "日本"; print -r -- "$REPLY""#)?;
    assert_eq!(out.trim(), "6:日本,", "UTF-8 should count bytes, not chars");
    Ok(())
}

#[test]
fn test_zsh_ns_matches_rust_encode() -> Result<(), Box<dyn std::error::Error>> {
    // Verify zsh encoding matches Rust encoding for various inputs
    let test_cases = [
        "",
        "hello",
        "日本語パス",
        "with spaces",
        "special:chars,here",
    ];
    for input in test_cases {
        let rust_encoded =
            String::from_utf8(capsule_protocol::netstring::encode(input.as_bytes()))?;
        let zsh_out = run_zsh_snippet(&format!(r#"_capsule_ns "{input}"; print -r -- "$REPLY""#))?;
        assert_eq!(
            zsh_out.trim(),
            rust_encoded,
            "zsh and Rust netstring encoding must match for input: {input:?}"
        );
    }
    Ok(())
}

// -- Pure function tests: _capsule_parse_wire ---------------------------------

#[test]
fn test_zsh_parse_wire_basic() -> Result<(), Box<dyn std::error::Error>> {
    // Wire format: netstring-encoded fields
    let out = run_zsh_snippet(
        r#"
_capsule_parse_wire "5:hello,5:world,"
print -r -- "${#_capsule_fields}"
print -r -- "${_capsule_fields[1]}"
print -r -- "${_capsule_fields[2]}"
"#,
    )?;
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[0], "2", "should parse 2 fields");
    assert_eq!(lines[1], "hello", "field 1");
    assert_eq!(lines[2], "world", "field 2");
    Ok(())
}

#[test]
fn test_zsh_parse_wire_empty_field() -> Result<(), Box<dyn std::error::Error>> {
    let out = run_zsh_snippet(
        r#"
_capsule_parse_wire "0:,5:hello,"
print -r -- "${#_capsule_fields}"
print -r -- "[${_capsule_fields[1]}]"
print -r -- "${_capsule_fields[2]}"
"#,
    )?;
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[0], "2", "should parse 2 fields");
    assert_eq!(lines[1], "[]", "field 1 should be empty");
    assert_eq!(lines[2], "hello", "field 2");
    Ok(())
}

#[test]
fn test_zsh_parse_wire_malformed_no_colon() -> Result<(), Box<dyn std::error::Error>> {
    // Malformed input with no colon should not hang (blocker fix verification)
    let out = run_zsh_snippet(
        r#"
_capsule_parse_wire "garbage"
print -r -- "${#_capsule_fields}"
"#,
    )?;
    assert_eq!(out.trim(), "0", "malformed input should produce 0 fields");
    Ok(())
}

#[test]
fn test_zsh_parse_wire_roundtrip_with_rust() -> Result<(), Box<dyn std::error::Error>> {
    // Build a wire message using Rust's netstring encoder, parse it in zsh
    let mut wire = Vec::new();
    capsule_protocol::netstring::encode_into(&mut wire, b"alpha");
    capsule_protocol::netstring::encode_into(&mut wire, b"beta");
    capsule_protocol::netstring::encode_into(&mut wire, b"");
    capsule_protocol::netstring::encode_into(&mut wire, "日本語".as_bytes());
    let wire_str = String::from_utf8(wire)?;

    let out = run_zsh_snippet(&format!(
        r#"
_capsule_parse_wire "{wire_str}"
print -r -- "${{#_capsule_fields}}"
for f in "${{_capsule_fields[@]}}"; do print -r -- "$f"; done
"#
    ))?;
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[0], "4", "should parse 4 fields");
    assert_eq!(lines[1], "alpha");
    assert_eq!(lines[2], "beta");
    assert_eq!(lines[3], "");
    assert_eq!(lines[4], "日本語");
    Ok(())
}

// -- zsh -n syntax check -----------------------------------------------------

#[test]
fn test_init_zsh_syntax_valid() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("zsh").args(["-n", INIT_ZSH]).output()?;
    assert!(
        output.status.success(),
        "zsh -n failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
