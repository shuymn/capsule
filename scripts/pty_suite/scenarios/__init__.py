"""Contract scenarios in run order; each takes the `Suite` and records its results."""

from __future__ import annotations

from .empty_enter import run_empty_enter
from .flicker import run_flicker
from .hooks import run_hook_errors, run_hook_matrix, run_hook_status, run_named_hook_error, run_preloaded_hook_helper
from .lifecycle import run_binary_selection, run_cleanup, run_functional, run_parallel

SCENARIOS = (
    run_binary_selection,
    run_functional,
    run_cleanup,
    run_parallel,
    run_empty_enter,
    run_hook_matrix,
    run_preloaded_hook_helper,
    run_hook_status,
    run_hook_errors,
    run_named_hook_error,
    run_flicker,
)
