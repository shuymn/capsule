//! Integration tests for `capsule init zsh`.

use std::process::Command;

/// Path to the init.zsh source file for direct zsh function testing.
const INIT_ZSH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/src/init/init.zsh");

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

// -- Verify removed protocol functions are absent ----------------------------

#[test]
fn test_init_zsh_no_netstring_functions() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .args(["init", "zsh"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("_capsule_ns"),
        "_capsule_ns should be removed"
    );
    assert!(
        !stdout.contains("_capsule_parse_wire"),
        "_capsule_parse_wire should be removed"
    );
    assert!(
        !stdout.contains("_capsule_send_request"),
        "_capsule_send_request should be removed"
    );
    Ok(())
}

// -- Tab-separated protocol format tests --------------------------------------

#[test]
fn test_zsh_precmd_sends_tab_separated_request() -> Result<(), Box<dyn std::error::Error>> {
    // Verify the precmd constructs a tab-separated request with correct field count.
    // We check the print format string in the source to ensure it uses \t separators.
    let output = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .args(["init", "zsh"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The precmd should use tab-separated format with print -nu
    assert!(
        stdout.contains(r#"\t${_CAPSULE_LAST_EXIT}\t"#),
        "precmd should use tab separators"
    );
    assert!(
        stdout.contains("print -nu $_CAPSULE_FD_IN"),
        "precmd should write to coproc fd"
    );
    Ok(())
}

#[test]
fn test_zsh_response_parsing_uses_tab_split() -> Result<(), Box<dyn std::error::Error>> {
    // Verify the async callback and precmd parse tab-separated responses
    let output = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .args(["init", "zsh"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check that response parsing uses tab-based field extraction
    assert!(
        stdout.contains(r#"${line%%$'\t'*}"#),
        "response parsing should extract first tab-delimited field"
    );
    assert!(
        stdout.contains(r#"${line#*$'\t'*$'\t'}"#),
        "response parsing should skip type and gen fields"
    );
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
