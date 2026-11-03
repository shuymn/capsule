"""Generated shell, acquisition, and configuration fixtures for disposable homes."""

from __future__ import annotations

import json
from pathlib import Path
import shlex
import subprocess

# Records the acquisition's view of cwd and exports, and optionally blocks on
# `$HOME/gate` or keeps a descendant alive for process-group cleanup checks.
ACQUIRE_SCRIPT = (
    "#!/bin/sh\n"
    'printf x >> "$HOME/count"\n'
    'printf "%s" "${CAP_VALUE-unset}" > "$HOME/value"\n'
    'printf "%s" "${CAP_REMOVE-unset}" > "$HOME/removed"\n'
    'printf "%s" "${CAP_PRIVATE-unset}" > "$HOME/private"\n'
    'printf "%s" "${CAP_RAW-unset}" > "$HOME/raw"\n'
    'pwd > "$HOME/cwd"\n'
    'if [ "${CAP_GATE-}" = 1 ]; then\n'
    '  printf ready > "$HOME/gate-ready"\n'
    '  read -r ignored < "$HOME/gate"\n'
    'fi\n'
    'if [ "${CAP_HOLD-}" = 1 ]; then\n'
    '  sleep 60 & descendant=$!\n'
    '  printf "%s %s" "$$" "$descendant" > "$HOME/children"\n'
    '  wait "$descendant"\n'
    'fi\n'
    'printf "VALUE=%s" "${CAP_VALUE-initial}"\n'
)


def acquire_config(script: Path) -> str:
    """Config whose only module runs ACQUIRE_SCRIPT."""
    return (
        "schema_version = 2\n[git]\ndisabled = true\n"
        "[[module]]\nname = 'fixture'\nformat = '{value}'\n"
        f"[module.values]\nvalue = [{{command = ['/bin/sh', {json.dumps(str(script))}]}}]\n"
    )


# Keys (bindkey caret notation) bound to the test widgets in both vi keymaps.
# Timeout for each fixture executable's setup run, before zsh starts.
SETUP_TIMEOUT_SECONDS = 8
STATE_KEY = "^O"  # dump BUFFER, PROMPT, and Capsule state to probe files
INJECT_KEY = "^P"  # feed $HOME/inject to _capsule_frame
CLEAR_KEY = "^U"  # clear the buffer and select viins
TEST_WIDGETS = (
    (STATE_KEY, "_capsule_test_state"),
    (INJECT_KEY, "_capsule_test_inject"),
    (CLEAR_KEY, "_capsule_test_clear"),
)

# Test instrumentation sourced after Capsule: transition logs, the ^O state
# probe, the ^P frame injector, the ^U line reset, and a line-init witness.
INSTRUMENTATION = (
    "_capsule_test_transition() {\n"
    "  local field\n"
    "  {\n"
    '    print -rn -- "$1"\n'
    '    for field in "$_CAPSULE_GENERATION" "$PROMPT" "$_CAPSULE_RENDERED" "$_CAPSULE_LAST_EXIT" "$_CAPSULE_DURATION_MS" "${COLUMNS:-80}" "${KEYMAP:-main}" "${_capsule_test_complete:-}"; do\n'
    r"      field=${field//\\/\\\\}" "\n"
    r"      field=${field//$'\t'/\\t}" "\n"
    r"      field=${field//$'\n'/\\n}" "\n"
    r"      field=${field//$'\r'/\\r}" "\n"
    "      print -rn -- $'\\t'\"$field\"\n"
    "    done\n"
    "    print\n"
    '  } >> "$HOME/transitions"\n'
    "}\n"
    "functions[_capsule_test_original_apply]=$functions[_capsule_apply_prompt]\n"
    "_capsule_apply_prompt() {\n"
    '  _capsule_test_original_apply "$@"\n'
    "  _capsule_test_transition apply\n"
    "}\n"
    "functions[_capsule_test_original_finalize]=$functions[_capsule_finalize_prompt]\n"
    "_capsule_finalize_prompt() {\n"
    '  _capsule_test_original_finalize "$@"\n'
    "  _capsule_test_transition precmd\n"
    "}\n"
    "functions[_capsule_test_original_callback]=$functions[_capsule_async_callback]\n"
    "_capsule_async_callback() {\n"
    '  _capsule_test_original_callback "$@"\n'
    "  _capsule_test_transition callback\n"
    "}\n"
    "functions[_capsule_test_original_frame]=$functions[_capsule_frame]\n"
    "_capsule_frame() {\n"
    "  local _capsule_test_complete=${1##*$'\\t'}\n"
    '  _capsule_test_original_frame "$@"\n'
    '  print -r -- "$1" >> "$HOME/frames"\n'
    "}\n"
    "_capsule_test_state() {\n"
    '  print -rn -- "$BUFFER" > "$HOME/buffer"\n'
    '  print -rn -- "$PROMPT" > "$HOME/prompt"\n'
    '  print -rn -- "$_CAPSULE_RENDERED" > "$HOME/rendered"\n'
    '  print -r -- "$CURSOR $_CAPSULE_GENERATION $_CAPSULE_COPROC_PID $_CAPSULE_FD_IN $_CAPSULE_FD_OUT ${#_CAPSULE_TX} ${#_CAPSULE_PENDING} $_CAPSULE_LAST_COLS $KEYMAP" > "$HOME/state.tmp"\n'
    '  command /bin/mv "$HOME/state.tmp" "$HOME/state"\n'
    "}\n"
    "_capsule_test_inject() {\n"
    '  _CAPSULE_CHANGED=0\n'
    '  _capsule_frame "$(< "$HOME/inject")"\n'
    '  (( _CAPSULE_CHANGED )) && zle reset-prompt\n'
    "}\n"
    "_capsule_test_line_init() {\n"
    "  _capsule_test_transition line-init\n"
    '  print -r -- "$_CAPSULE_GENERATION" > "$HOME/zle-ready"\n'
    "}\n"
    "add-zle-hook-widget line-init _capsule_test_line_init\n"
    "_capsule_test_clear() { BUFFER=''; CURSOR=0; zle -K viins; }\n"
    + "".join(
        f"zle -N {widget}\nbindkey -M viins '{key}' {widget}\nbindkey -M vicmd '{key}' {widget}\n"
        for key, widget in TEST_WIDGETS
    )
)


def zshrc(path: str, precmd_setup: str = "") -> str:
    """Vi-mode `.zshrc` that installs user hooks, Capsule, then INSTRUMENTATION."""
    return (
        f"export PATH={shlex.quote(path)}\n"
        "unsetopt beep checkjobs\n"
        "setopt promptsubst\n"
        "bindkey -v\n"
        "KEYTIMEOUT=1\n"
        + precmd_setup
        + 'source "$HOME/init.zsh"\n'
        + INSTRUMENTATION
    )


# A conflicting `capsule` beside the renamed selected binary.
CONFLICTING_CAPSULE = '#!/bin/sh\nprintf unexpected > "$HOME/wrong-binary"\nexit 97\n'

# Replaces the shell via exec and stays alive until the gate releases it.
REPLACEMENT_SCRIPT = (
    "import os\nfrom pathlib import Path\n"
    'home = Path(os.environ["HOME"])\n'
    '(home / "replacement-ready").write_text(str(os.getpid()))\n'
    'with (home / "replacement-gate").open() as gate:\n'
    '    assert gate.readline() == "release\\n"\n'
)


def flicker_fixture(home: Path) -> str:
    """File-controlled gates keep ordinary commands' exported snapshots identical."""
    (home / "work/.git").mkdir()
    (home / "bin").mkdir()
    for name in ("git", "tool"):
        script = home / "bin" / name
        action = (
            'printf "# branch.head %s\\n" "$mode"\n'
            if name == "git" else 'printf "%s" "$mode"\n'
        )
        script.write_text(
            "#!/bin/sh\n"
            + ('printf x >> "$HOME/count"\n' if name == "tool" else "")
            + f'mode=$(/bin/cat "$HOME/{name}.mode")\n'
            + f'if [ -f "$HOME/{name}.armed" ]; then\n'
            + f'  printf ready > "$HOME/{name}.ready"\n'
            + f'  read -r ignored < "$HOME/{name}.gate"\n'
            + "fi\n"
            + 'if [ "$mode" = failed ]; then\n  result=7\nelse\n'
            + action
            + "result=$?\nfi\n"
            + f'printf done > "$HOME/{name}.done"\n'
            + 'exit "$result"\n'
        )
        script.chmod(0o755)
        (home / f"{name}.mode").write_text("PTY_BRANCH_OLD" if name == "git" else "TOOL_OLD")
    (home / "work/version").write_text("FILE_OLD")
    (home / "work/condition").write_text("present")
    # Prepare fresh executables before zsh's first precmd, outside acquisition
    # deadlines. Setup must not leave counts or generation-completion witnesses.
    for name, expected in (("git", b"# branch.head PTY_BRANCH_OLD\n"), ("tool", b"TOOL_OLD")):
        result = subprocess.run(
            [str(home / "bin" / name)],
            cwd=home / "work",
            env={"HOME": str(home), "PATH": f"{home / 'bin'}:/usr/bin:/bin"},
            input=b"", capture_output=True, check=True,
            timeout=SETUP_TIMEOUT_SECONDS,
        )
        assert result.stdout == expected, (name, result.stdout, result.stderr)
        assert (home / f"{name}.done").read_bytes() == b"done", name
    assert (home / "count").read_bytes() == b"x", "fixture setup must run tool exactly once"
    for name in ("count", "git.done", "tool.done"):
        (home / name).unlink()
    return (
        "schema_version = 2\n"
        "[git]\nicon = ''\nconnector = ''\n"
        "[cmd_duration]\nthreshold_ms = 0\nconnector = 'DURATION'\n"
        "[character]\nglyph = 'INPUT'\n"
        "success_style = {fg = 'green', bold = false}\n"
        "error_style = {fg = 'red', bold = false}\n"
        "[character.vicmd]\nglyph = 'NORMAL'\nstyle = {fg = 'blue', bold = false}\n"
        "[[module]]\nname = 'tool'\nformat = '{value}'\n"
        "[module.values]\nvalue = [{command = ['tool']}]\n"
        "[[module]]\nname = 'file'\nformat = '{value}'\n"
        "[module.values]\nvalue = [{file = 'version'}]\n"
        "[[module]]\nname = 'conditional'\nwhen.files = ['condition']\nformat = 'COND_OLD'\nvalues = {}\n"
        "[[module]]\nname = 'environment'\nformat = 'ENV={value}'\n"
        "[module.values]\nvalue = [{env = 'CAP_FLICKER'}]\n"
    )


EMPTY_CONFIG = "schema_version = 2\n"

DATA_FILE_CONFIG = (
    "schema_version = 2\n[git]\ndisabled = true\n"
    "[[module]]\nname = 'data'\nformat = '{value}'\n"
    "[module.values]\nvalue = [{file = 'data'}]\n"
)

FOO_FILE_CONFIG = (
    "schema_version = 2\n"
    "[[module]]\nname = 'file'\nwhen.files = ['foo.md']\nformat = 'FILE={value}'\n"
    "[module.values]\nvalue = [{file = 'foo.md'}]\n"
)

# Command-substitution payload that must stay literal in prompts.
MATRIX_PAYLOAD = "$(print -n x > ~/dollar) `print -n x > ~/backtick`"
MATRIX_SETUP = (
    f"export CAP_VALUE={shlex.quote(MATRIX_PAYLOAD)}\n"
    "typeset -gi _capsule_test_count=0\n"
    "_capsule_test_matrix() {\n"
    "  local action\n"
    '  [[ -s $HOME/matrix-action ]] || return 0\n'
    '  read -r action < $HOME/matrix-action\n'
    "  [[ $action == $1:* ]] || return 0\n"
    '  : > $HOME/matrix-action\n'
    "  case ${action#*:} in\n"
    '    cd) if [[ $PWD == $HOME/work ]]; then cd -- $HOME/work/matrix; else cd -- $HOME/work; fi ;;\n'
    f'    export) export CAP_VALUE={shlex.quote(MATRIX_PAYLOAD)}" $((++_capsule_test_count))" ;;\n'
    "    setopt) if [[ -o promptsubst ]]; then unsetopt promptsubst; else setopt promptsubst; fi ;;\n"
    "    error) : $((1/0)) ;;\n"
    "  esac\n"
    "}\n"
    "precmd() { _capsule_test_matrix named; }\n"
    "_capsule_test_matrix_hook() { _capsule_test_matrix hook; }\n"
    "precmd_functions=(_capsule_test_matrix_hook)\n"
)

PRELOADED_HOOK_SETUP = (
    "autoload -Uz add-zsh-hook add-zle-hook-widget\n"
    "_capsule_test_existing_hook() { :; }\n"
    "add-zsh-hook precmd _capsule_test_existing_hook\n"
)

HOOK_STATUS_SETUP = (
    "_capsule_test_first_status() { "
    'print -r -- "$?" > "$HOME/first-status"; return 7; }\n'
    "_capsule_test_last_status() { "
    'print -r -- "$?" > "$HOME/last-status"; }\n'
    "precmd_functions=(_capsule_test_first_status _capsule_test_last_status)\n"
)

HOOK_ERROR_SETUP = (
    "_capsule_test_user_hook() {\n"
    "  if (( _capsule_test_hook_error )); then\n"
    "    [[ $_capsule_test_hook_subst == on ]] && setopt promptsubst\n"
    "    print -r -- reached > \"$HOME/error-hook\"\n"
    "    : $((1/0))\n"
    "  fi\n"
    "  return 0\n"
    "}\n"
    "_capsule_test_late_hook() { print -r -- reached > \"$HOME/late-hook\"; }\n"
    "precmd_functions=(_capsule_test_user_hook _capsule_test_late_hook)\n"
)

NAMED_HOOK_ERROR_SETUP = (
    "precmd() {\n"
    "  if (( _capsule_test_named_error )); then\n"
    "    _capsule_test_named_error=0\n"
    "    setopt promptsubst\n"
    "    print -r -- reached > \"$HOME/named-error\"\n"
    "    : $((1/0))\n"
    "  fi\n"
    "  return 0\n"
    "}\n"
)
