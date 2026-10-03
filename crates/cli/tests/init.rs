//! Evaluate the public initialization output in an isolated zsh.

use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn generated_init_preserves_user_hooks_and_can_be_evaluated_twice()
-> Result<(), Box<dyn std::error::Error>> {
    let init = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .args(["init", "zsh"])
        .output()?;
    assert!(init.status.success());
    let tmp = tempfile::tempdir()?;
    let mut child = Command::new("zsh")
        .args(["-f"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").ok_or("missing PATH")?)
        .env("HOME", tmp.path())
        .env("ZDOTDIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().ok_or("missing zsh stdin")?;
    input.write_all(b"precmd_functions=(user_precmd)\npreexec_functions=(user_preexec)\nzshexit_functions=(user_exit)\nuser_exit() { :; }\n")?;
    input.write_all(&init.stdout)?;
    input.write_all(&init.stdout)?;
    input.write_all(
        br#"
[[ ${precmd_functions[*]} == '_capsule_precmd _capsule_run_precmd_hooks' ]] || exit 11
[[ ${_CAPSULE_PRECMD_HOOKS[*]} == user_precmd ]] || exit 16
[[ ${preexec_functions[*]} == '_capsule_preexec user_preexec' ]] || exit 12
[[ ${zshexit_functions[*]} == '_capsule_cleanup_fds user_exit' ]] || exit 13
[[ -n $PROMPT ]] || exit 14
(( !_CAPSULE_COPROC_PID && !_CAPSULE_FD_IN && !_CAPSULE_FD_OUT )) || exit 15

# Standard hook registration/removal must still reach the captured user hooks
# after repeated initialization, including listing and pattern removal.
user_precmd() { order+=first; return 7; }
late_precmd() { order+=last; }
add-zsh-hook precmd late_precmd
add-zsh-hook precmd late_precmd
[[ ${_CAPSULE_PRECMD_HOOKS[*]} == 'user_precmd late_precmd' ]] || exit 17
add-zsh-hook -d precmd user_precmd
[[ ${_CAPSULE_PRECMD_HOOKS[*]} == late_precmd ]] || exit 18
listed=$(add-zsh-hook -L precmd)
[[ $listed == *late_precmd* && $listed != *user_precmd* ]] || exit 19
add-zsh-hook -D precmd 'late_*'
(( ! ${#_CAPSULE_PRECMD_HOOKS} )) || exit 20
add-zsh-hook precmd user_precmd
add-zsh-hook precmd late_precmd
_capsule_finalize_prompt() { finalized=1; }
order='' finalized=0
_capsule_run_precmd_hooks
[[ $order == firstlast && $finalized == 1 ]] || exit 21
[[ ${precmd_functions[*]} == '_capsule_precmd _capsule_run_precmd_hooks' ]] || exit 22
"#,
    )?;
    drop(input);
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "zsh: {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
