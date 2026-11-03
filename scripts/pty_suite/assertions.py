"""Expectations over every recorded render and prompt transition of a generation."""

from __future__ import annotations

import re

from .harness import eventually

# Display markers produced by the flicker fixture's external (acquired) modules.
EXTERNAL_MARKERS = {
    b"PTY_BRANCH_OLD", b"PTY_BRANCH_NEW", b"TOOL_OLD", b"TOOL_NEW",
    b"FILE_OLD", b"COND_OLD", b"ENV=exported", b"RELOADED-",
}


def external(prompt):
    return {marker for marker in EXTERNAL_MARKERS if marker in prompt}


def display_width(line: bytes) -> int:
    """Width of one prompt line with zsh `%{...%}` escapes removed."""
    return len(re.sub(rb"%\{.*?%\}", b"", line).decode())


def assert_history(shell, mark, generation, pending, settled=None):
    """Check every emitted render and actual prompt, including precmd fallback."""
    renders = shell.renders(mark, generation)
    transitions = shell.transitions(mark, generation)
    assert renders and any(item["event"] == b"precmd" for item in transitions)
    if settled is not None:
        assert any(item["event"] == b"apply" and item["complete"] == b"1" for item in transitions), ("missing accepted completion", generation, transitions)
    settled = pending if settled is None else settled
    for prompt, complete in renders:
        expected = settled if complete else pending
        assert external(prompt) == expected, ("render", generation, complete, expected, prompt)
    for transition in transitions:
        if transition["event"] == b"apply":
            expected = settled if transition["complete"] == b"1" else pending
            assert external(transition["effective"]) == expected, ("apply", generation, expected, transition)
        else:
            # Callbacks can finish a batch containing the settled frame.
            assert external(transition["effective"]) in (pending, settled), ("prompt transition", generation, pending, transition)


def wait_pending(shell, mark, generation, predicate):
    eventually(
        lambda: any(not complete and predicate(prompt) for prompt, complete in shell.renders(mark, generation)),
        "local state rendered while acquisition is pending", shell.pump, timeout=0.2,
    )
