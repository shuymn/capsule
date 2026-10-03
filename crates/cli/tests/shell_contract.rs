//! Shell response boundaries without service startup or real user configuration.

use std::{
    process::{Command, Stdio},
    time::Duration,
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const INIT: &str = include_str!("../../core/src/init/init.zsh");
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn functions() -> String {
    format!(
        "zle() {{ :; }}\n{}\nzmodload zsh/system\nzmodload zsh/zselect\n\
        typeset -gi ERRNO=0 _CAPSULE_GENERATION=2 _CAPSULE_FD_IN=0 _CAPSULE_FD_OUT=0\n\
        typeset -g _CAPSULE_RX='' _CAPSULE_TX='' _CAPSULE_PENDING='' _CAPSULE_FALLBACK=fallback _CAPSULE_PROMPT_SUBST=off\n\
        PROMPT=unchanged\n",
        INIT.replace("\n_capsule_init\n", "\n")
    )
}

#[test]
fn responses_must_match_exact_generation() -> Result {
    let home = tempfile::tempdir()?;
    let script = format!(
        "{}\n\
        _capsule_frame $'R\\t1\\tstale\\tcharacter\\t1'\n\
        [[ $PROMPT == unchanged ]] || exit 11\n\
        _capsule_frame $'R\\t3\\tfuture\\tcharacter\\t1'\n\
        [[ $PROMPT == unchanged ]] || exit 12\n\
        _capsule_frame $'R\\t2\\tcurrent\\tcharacter\\t1'\n\
        [[ $PROMPT == $'current\\ncharacter ' ]]",
        functions()
    );
    let result = Command::new("zsh")
        .args(["-f", "-c", &script])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
        .env("HOME", home.path())
        .env("ZDOTDIR", home.path())
        .output()?;
    assert!(
        result.status.success(),
        "generation check failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

#[test]
fn unchanged_display_skips_redraw_but_restores_fallback() -> Result {
    let home = tempfile::tempdir()?;
    let script = format!(
        "{}\n\
        for _CAPSULE_PROMPT_SUBST in off on; do\n\
            PROMPT=$_CAPSULE_FALLBACK\n\
            _CAPSULE_CHANGED=0\n\
            _capsule_frame $'R\\t2\\tcurrent\\tcharacter\\t0'\n\
            (( _CAPSULE_CHANGED == 1 )) || exit 11\n\
            original=$PROMPT\n\
            _CAPSULE_CHANGED=0\n\
            _capsule_frame $'R\\t2\\tcurrent\\tcharacter\\t1'\n\
            (( _CAPSULE_CHANGED == 0 )) || exit 12\n\
            [[ $PROMPT == $original ]] || exit 13\n\
            PROMPT=$_CAPSULE_FALLBACK\n\
            _capsule_frame $'R\\t2\\tcurrent\\tcharacter\\t1'\n\
            (( _CAPSULE_CHANGED == 1 )) || exit 14\n\
            [[ $PROMPT == $original ]] || exit 15\n\
            _CAPSULE_CHANGED=0\n\
            _capsule_frame $'R\\t2\\tupdated\\tcharacter\\t1'\n\
            (( _CAPSULE_CHANGED == 1 )) || exit 16\n\
            [[ $_CAPSULE_RENDERED == $'updated\\ncharacter ' ]] || exit 17\n\
        done",
        functions()
    );
    let result = Command::new("zsh")
        .args(["-f", "-c", &script])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
        .env("HOME", home.path())
        .env("ZDOTDIR", home.path())
        .output()?;
    assert!(
        result.status.success(),
        "redraw check failed ({}): {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn partial_response_returns_before_the_remaining_bytes_arrive() -> Result {
    let home = tempfile::tempdir()?;
    let script = format!(
        "{}\n\
        exec {{_CAPSULE_FD_OUT}}<&0\n\
        command \"$CAPSULE_TEST_BIN\" fd-config <&$_CAPSULE_FD_OUT >/dev/null || exit 10\n\
        zselect -r $_CAPSULE_FD_OUT -t 300 || exit 11\n\
        _capsule_async_callback\n\
        [[ $PROMPT == unchanged && $_CAPSULE_RX == $'R\\t2\\tcur' ]] || exit 12\n\
        print -r -- PARTIAL\n\
        zselect -r $_CAPSULE_FD_OUT -t 300 || exit 13\n\
        _capsule_async_callback\n\
        [[ $PROMPT == $'current\\ncharacter ' ]] || exit 14\n\
        print -r -- COMPLETE\n",
        functions()
    );
    let mut child = tokio::process::Command::new("zsh")
        .args(["-f", "-c", &script])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
        .env("HOME", home.path())
        .env("ZDOTDIR", home.path())
        .env("CAPSULE_TEST_BIN", env!("CARGO_BIN_EXE_capsule"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = child.stdin.take().ok_or("missing stdin")?;
    let mut output = BufReader::new(child.stdout.take().ok_or("missing stdout")?).lines();
    input.write_all(b"R\t2\tcur").await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(4), output.next_line())
            .await??
            .as_deref(),
        Some("PARTIAL")
    );
    input.write_all(b"rent\tcharacter\t1\n").await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(4), output.next_line())
            .await??
            .as_deref(),
        Some("COMPLETE")
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(4), child.wait())
            .await??
            .success()
    );
    Ok(())
}
