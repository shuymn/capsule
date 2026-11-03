"""Real-PTY zsh drivers shared by the session contract and the latency benchmark.

- `terminal`: spawn zsh in a PTY and size its window (shared with prompt_latency.py)
- `harness`: the instrumented `Shell` DSL, FIFO `Gates`, and the `Suite` runner
- `fixtures`: generated `.zshrc`, acquisition scripts, configs, and hook setups
- `assertions`: shared expectations over recorded renders and prompt transitions
- `scenarios`: the contract scenarios, run in order by session_pty.py
"""
