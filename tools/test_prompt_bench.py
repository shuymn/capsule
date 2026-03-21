"""Unit tests for the prompt benchmark helpers."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest


MODULE_PATH = Path(__file__).with_name("prompt_bench.py")
SPEC = importlib.util.spec_from_file_location("prompt_bench", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("failed to load prompt_bench module")
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class PercentileTests(unittest.TestCase):
    def test_percentile_interpolates(self) -> None:
        self.assertAlmostEqual(MODULE.percentile([10.0, 20.0, 30.0, 40.0], 0.95), 38.5)


class SummaryTests(unittest.TestCase):
    def test_summarize_reports_expected_fields(self) -> None:
        summary = MODULE.summarize([10.0, 20.0, 30.0])

        self.assertEqual(summary.count, 3)
        self.assertEqual(summary.min_ms, 10.0)
        self.assertEqual(summary.p50_ms, 20.0)
        self.assertEqual(summary.max_ms, 30.0)


class PathTests(unittest.TestCase):
    def test_build_minimal_path_deduplicates_directories(self) -> None:
        path = MODULE.build_minimal_path(
            [
                "/opt/homebrew/bin/starship",
                "/opt/homebrew/bin/git",
                "/bin/zsh",
            ]
        )

        parts = path.split(":")
        self.assertIn("/bin", parts)
        self.assertIn("/usr/bin", parts)
        self.assertEqual(len(parts), len(set(parts)))


class ZshrcTests(unittest.TestCase):
    def test_render_zshrc_adds_marker_hook(self) -> None:
        zshrc = MODULE.render_zshrc(
            "capsule",
            "/tmp/capsule",
            "/tmp/starship",
        )

        self.assertIn("precmd_functions+=(_prompt_bench_notify)", zshrc)
        self.assertIn(MODULE.MARKER_PREFIX, zshrc)


if __name__ == "__main__":
    unittest.main()
