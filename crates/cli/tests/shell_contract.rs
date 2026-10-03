//! Shell response boundaries without service startup or real user configuration.

use std::{
    process::{Command, Stdio},
    time::Duration,
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const INIT: &str = include_str!("../../core/src/init/init.zsh");
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn spawn_zsh(script: &str, home: &std::path::Path) -> Result<tokio::process::Child> {
    Ok(tokio::process::Command::new("zsh")
        .args(["-f", "-c", script])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
        .env("HOME", home)
        .env("ZDOTDIR", home)
        .env("CAPSULE_TEST_BIN", env!("CARGO_BIN_EXE_capsule"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?)
}

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
async fn retained_prompt_tracks_user_hook_option_changes_before_a_response() -> Result {
    let home = tempfile::tempdir()?;
    let script = format!(
        "zle() {{ :; }}\n\
        precmd_functions=(user_precmd)\n\
        user_precmd() {{ if [[ $next_subst == on ]]; then setopt promptsubst; else unsetopt promptsubst; fi; }}\n\
        {INIT}\nzmodload zsh/zselect\n{}",
        r#"
[[ ${precmd_functions[*]} == '_capsule_precmd user_precmd _capsule_finalize_prompt' ]] || exit 9
exec {_CAPSULE_FD_OUT}<&0 {_CAPSULE_FD_IN}>/dev/null
command "$CAPSULE_TEST_BIN" fd-config <&$_CAPSULE_FD_OUT >&$_CAPSULE_FD_IN || exit 10
unsetopt promptsubst
_CAPSULE_PROMPT_SUBST=off _CAPSULE_GENERATION=2
payload='$(touch "$HOME/dollar") `touch "$HOME/backtick"`'
expected=$payload$'\ncharacter '
_capsule_frame $'R\t2\t'"$payload"$'\tcharacter\t1'
[[ $PROMPT == "$expected" ]] || exit 11
_CAPSULE_CWD=$PWD _CAPSULE_CMD_START=''
_capsule_snapshot

# No response is available until both post-Capsule option transitions expand.
next_subst=on
_CAPSULE_NEED_GENERATION=1
for hook in "${precmd_functions[@]}"; do "$hook" >/dev/null || exit 20; done
[[ ${options[promptsubst]} == on && $_CAPSULE_PROMPT_SUBST == on ]] || exit 12
expanded=$(print -P -r -- "$PROMPT")
[[ ! -e "$HOME/dollar" && ! -e "$HOME/backtick" ]] || exit 13
[[ $PROMPT == '${_CAPSULE_RENDERED}' && $expanded == "$expected" ]] || exit 14

next_subst=off
_CAPSULE_NEED_GENERATION=1
for hook in "${precmd_functions[@]}"; do "$hook" >/dev/null || exit 21; done
[[ ${options[promptsubst]} == off && $_CAPSULE_PROMPT_SUBST == off ]] || exit 15
expanded=$(print -P -r -- "$PROMPT")
[[ ! -e "$HOME/dollar" && ! -e "$HOME/backtick" ]] || exit 16
[[ $PROMPT == "$expected" && $expanded == "$expected" ]] || exit 17

# The final hook must not seize fallback or a prompt assigned by a user hook.
for preserved in "$_CAPSULE_FALLBACK" 'user-owned'; do
    PROMPT=$preserved
    setopt promptsubst
    _capsule_finalize_prompt
    [[ $PROMPT == "$preserved" ]] || exit 22
    unsetopt promptsubst
    _capsule_finalize_prompt
    [[ $PROMPT == "$preserved" ]] || exit 23
done
PROMPT=$_CAPSULE_RENDERED
print -r -- SAFE

zselect -r $_CAPSULE_FD_OUT -t 300 || exit 18
_capsule_async_callback
[[ $PROMPT == $'updated\ncharacter ' ]] || exit 19
print -r -- COMPLETE
"#
    );
    let mut child = spawn_zsh(&script, home.path())?;
    let mut input = child.stdin.take().ok_or("missing stdin")?;
    let mut output = BufReader::new(child.stdout.take().ok_or("missing stdout")?).lines();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(4), output.next_line())
            .await??
            .as_deref(),
        Some("SAFE")
    );
    input.write_all(b"R\t4\tupdated\tcharacter\t1\n").await?;
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

#[test]
fn failed_redraw_write_selects_fallback_after_transport_cleanup() -> Result {
    let home = tempfile::tempdir()?;
    let script = format!(
        "{}\n\
        zle() {{ [[ $1 == reset-prompt ]] && redraw=1; }}\n\
        _CAPSULE_SNAPSHOT=/tmp _CAPSULE_LAST_COLS='' _CAPSULE_LAST_KEYMAP=''\n\
        # An invalid descriptor makes syswrite fail before any bytes are sent.\n\
        _CAPSULE_FD_IN=999\n\
        _capsule_redraw\n\
        [[ $PROMPT == fallback ]] || exit 11\n\
        (( !_CAPSULE_FD_IN && !_CAPSULE_FD_OUT && _CAPSULE_NEED_GENERATION )) || exit 12\n\
        (( redraw == 1 )) || exit 13",
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
        "failed write check failed ({}): {}",
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
    let mut child = spawn_zsh(&script, home.path())?;
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
