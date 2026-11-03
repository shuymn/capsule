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


def assert_history(prompt, pending, settled=None):
    """Check every emitted render and actual prompt, including precmd fallback."""
    renders = prompt.renders()
    transitions = prompt.transitions()
    generation = prompt.generation
    assert renders and prompt.finalized()
    if settled is not None:
        assert prompt.applied(complete=True), ("missing accepted completion", generation, transitions)
    settled = pending if settled is None else settled
    for text, complete in renders:
        expected = settled if complete else pending
        assert external(text) == expected, ("render", generation, complete, expected, text)
    for transition in transitions:
        if transition["event"] == b"apply":
            expected = settled if transition["complete"] == b"1" else pending
            assert external(transition["effective"]) == expected, ("apply", generation, expected, transition)
        else:
            # Callbacks can finish a batch containing the settled frame.
            assert external(transition["effective"]) in (pending, settled), ("prompt transition", generation, pending, transition)


def wait_pending(prompt, predicate):
    eventually(
        lambda: any(not complete and predicate(text) for text, complete in prompt.renders()),
        "local state rendered while acquisition is pending", prompt.shell.pump, timeout=0.2,
    )


def rerender(prompt, action, predicate):
    """Run a local display `action`, probe ZLE, and wait for a matching pending render.

    Returns the probe's state and the prompt scoped to records from before `action`.
    """
    redraw = prompt.from_now()
    action()
    state = prompt.shell.state()  # Also triggers zsh's line-pre-redraw hook.
    wait_pending(redraw, predicate)
    return state, redraw
