//! Integration tests for `capsule init zsh`.

use std::process::Command;

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
