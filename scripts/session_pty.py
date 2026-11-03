#!/usr/bin/env python3
"""Exercise the real zsh/worker boundary in disposable homes (no benchmarks).

Run with `mise run test:pty` (builds, then uses uv with the pinned .python-version).
Use --output <new-directory> to create a private artifact directory.
Use --describe to print the contract table embedded in docs/testing.md.
The harness, scenarios, and contract registry live in the sibling `pty_suite` package.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import tempfile

from pty_suite.contracts import markdown_table
from pty_suite.harness import Suite
from pty_suite.scenarios import SCENARIOS


def run(binary: Path, output: Path):
    suite = Suite(binary, output)
    try:
        for scenario in SCENARIOS:
            scenario(suite)
        suite.require_all_records()
    finally:
        suite.finish()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/capsule"))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--describe", action="store_true", help="print the contract table and exit")
    args = parser.parse_args()
    if args.describe:
        print(markdown_table(), end="")
        return
    if args.output is None:
        output = Path(tempfile.mkdtemp(prefix="capsule-pty-"))
    else:
        output = args.output
        try:
            output.mkdir(mode=0o700, parents=True)
        except FileExistsError:
            parser.error(f"output directory already exists: {output}")
        except OSError as error:
            parser.error(f"cannot create output directory: {error}")
    print(f"PTY artifacts: {output}", flush=True)
    run(args.binary.resolve(), output.resolve())


if __name__ == "__main__":
    main()
