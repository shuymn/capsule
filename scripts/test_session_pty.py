"""Regressions for the PTY driver's CLI boundary and pure harness helpers."""

import ast
from pathlib import Path
import os
import stat
import subprocess
import sys
import tempfile
import unittest

from pty_suite import contracts
from pty_suite.assertions import display_width, external
from pty_suite.harness import SUBST_PROMPT, frame_record, parse_state, parse_transition, unescape
from pty_suite.scenarios.hooks import MATRIX_CASES


DRIVER = Path(__file__).with_name("session_pty.py")
SCENARIOS = Path(__file__).with_name("pty_suite") / "scenarios"
TESTING_DOC = Path(__file__).parent.parent / "docs" / "testing.md"


def run_driver(*arguments, env=None, umask=-1):
    return subprocess.run(
        [sys.executable, str(DRIVER), *map(str, arguments)],
        capture_output=True, timeout=5, env=env, umask=umask,
    )


class OutputDirectoryTests(unittest.TestCase):
    def test_existing_output_does_not_overwrite_a_symlink_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            victim = root / "dummy-victim"
            victim.write_text("keep this dummy file\n")
            output = root / "output"
            output.mkdir()
            (output / "results.json").symlink_to(victim)
            # Force an error before any PTY or selected binary can start.
            (output / "binary-selection-bin").mkdir()
            result = run_driver("--output", output)
            self.assertEqual(victim.read_text(), "keep this dummy file\n")
            self.assertEqual(result.returncode, 2, result.stderr.decode())
            self.assertIn(b"output directory already exists", result.stderr)
            self.assertEqual(
                sorted(path.name for path in output.iterdir()),
                ["binary-selection-bin", "results.json"],
            )

    def test_existing_files_and_symlinks_are_rejected_without_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "directory"
            directory.mkdir()
            (directory / "binary-selection-bin").mkdir()
            marker = directory / "marker"
            marker.write_text("keep this marker\n")
            existing_file = root / "file"
            existing_file.write_text("keep this file\n")
            link = root / "link"
            link.symlink_to(directory, target_is_directory=True)
            dangling_link = root / "dangling-link"
            dangling_link.symlink_to(root / "absent", target_is_directory=True)
            for output in (existing_file, link, dangling_link):
                with self.subTest(output=output.name):
                    result = run_driver("--output", output)
                    self.assertEqual(result.returncode, 2, result.stderr.decode())
                    self.assertIn(b"output directory already exists", result.stderr)
            self.assertEqual(existing_file.read_text(), "keep this file\n")
            self.assertEqual(marker.read_text(), "keep this marker\n")
            self.assertEqual(
                sorted(path.name for path in directory.iterdir()),
                ["binary-selection-bin", "marker"],
            )
            self.assertEqual(link.readlink(), directory)
            self.assertEqual(dangling_link.readlink(), root / "absent")
            self.assertFalse((root / "absent").exists())

    def test_new_output_is_private_even_with_a_permissive_umask(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "parent" / "new-output"
            result = run_driver(
                "--output", output, "--binary", root / "absent-binary", umask=0,
            )
            # The absent binary stops the run before a PTY starts; its error
            # still leaves diagnostics inside the exclusively created directory.
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"PTY artifacts:", result.stdout)
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
            self.assertEqual((output / "results.json").read_text(), "[]\n")

    def test_default_output_remains_a_new_private_temporary_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = run_driver(
                "--binary", root / "absent-binary", umask=0,
                env={**os.environ, "TMPDIR": str(root)},
            )
            self.assertNotEqual(result.returncode, 0)
            line = result.stdout.decode().splitlines()[0]
            self.assertTrue(line.startswith("PTY artifacts: "), line)
            output = Path(line.removeprefix("PTY artifacts: "))
            self.assertEqual(output.parent.resolve(), root.resolve())
            self.assertTrue(output.name.startswith("capsule-pty-"))
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
            self.assertEqual((output / "results.json").read_text(), "[]\n")


class HarnessTests(unittest.TestCase):
    def test_unescape_reverses_transition_log_escapes(self):
        self.assertEqual(unescape(b"a\\\\b\\tc\\nd\\re\\x"), b"a\\b\tc\nd\re\\x")

    def test_frame_record_matches_the_worker_frame_layout(self):
        self.assertEqual(frame_record(4, "line one", b"two"), b"R\t4\tline one\ttwo\t1")
        self.assertEqual(frame_record(5, "a", "b", complete=False), b"R\t5\ta\tb\t0")

    def test_parse_state_reads_numeric_fields_and_keymap(self):
        state = parse_state("3 7 1234 10 11 0 2 100 vicmd\n")
        self.assertEqual(state, {
            "cursor": 3, "generation": 7, "worker": 1234, "write_fd": 10, "read_fd": 11,
            "tx": 0, "pending": 2, "cols": 100, "keymap": "vicmd",
        })

    def test_parse_transition_resolves_the_effective_prompt(self):
        subst = parse_transition(b"apply\t2\t" + SUBST_PROMPT + b"\tline\\nnext \t7\t\t80\tmain\t1")
        self.assertEqual((subst["generation"], subst["effective"]), (2, b"line\nnext "))
        literal = parse_transition(b"precmd\t3\tretained\trendered\t0\t3000\t80\tviins\t")
        self.assertEqual((literal["effective"], literal["duration"]), (b"retained", b"3000"))

    def test_external_markers_and_display_width_ignore_escapes(self):
        self.assertEqual(external(b"TOOL_OLD ENV=exported x"), {b"TOOL_OLD", b"ENV=exported"})
        self.assertEqual(display_width(b"%{\x1b[31m%}INPUT%{\x1b[0m%}"), 5)

    def test_hook_matrix_covers_every_action_for_both_triggers(self):
        self.assertEqual(len(MATRIX_CASES), 16)
        self.assertEqual(len(set(MATRIX_CASES)), 16)


def scenario_records():
    """Literal record names passed to `record(...)` in every scenario module."""
    names = []
    for path in sorted(SCENARIOS.glob("*.py")):
        for node in ast.walk(ast.parse(path.read_text(), str(path))):
            is_record = (
                isinstance(node, ast.Call)
                and getattr(node.func, "attr", getattr(node.func, "id", None)) == "record"
            )
            if not is_record:
                continue
            constants = [
                item.value for item in ast.walk(node.args[0])
                if isinstance(item, ast.Constant) and isinstance(item.value, str)
            ]
            dynamic = [
                item for item in ast.walk(node.args[0])
                if isinstance(item, (ast.Name, ast.JoinedStr, ast.BinOp, ast.Call))
            ]
            if dynamic or not constants:
                raise AssertionError(f"{path.name}:{node.lineno}: record names must be string literals")
            names.extend(constants)
    return names


class ContractRegistryTests(unittest.TestCase):
    def test_every_scenario_record_is_registered_exactly_once(self):
        emitted = scenario_records()
        self.assertEqual(sorted(emitted), sorted(set(emitted)), "a record name is emitted twice")
        self.assertEqual(set(emitted), contracts.RECORDS)
        registered = [name for contract in contracts.CONTRACTS for name in contract.records]
        self.assertEqual(len(registered), len(contracts.RECORDS), "a record is listed under two scopes")

    def test_contracts_have_unique_scopes_and_descriptions(self):
        scopes = [contract.scope for contract in contracts.CONTRACTS]
        self.assertEqual(len(scopes), len(set(scopes)))
        for contract in contracts.CONTRACTS:
            self.assertTrue(contract.behavior and contract.records, contract.scope)
            self.assertNotIn("|", contract.behavior, contract.scope)

    def test_missing_records_are_reported_in_registry_order(self):
        recorded = set(contracts.RECORDS) - {"cwd_bytes_reach_acquisition", "ten_shells_own_ten_workers_and_leave_none_active"}
        self.assertEqual(
            contracts.missing_records(recorded),
            ["cwd_bytes_reach_acquisition", "ten_shells_own_ten_workers_and_leave_none_active"],
        )

    def test_testing_doc_embeds_the_generated_contract_table(self):
        document = TESTING_DOC.read_text()
        begin = document.index(contracts.TABLE_BEGIN) + len(contracts.TABLE_BEGIN) + 1
        end = document.index(contracts.TABLE_END)
        self.assertEqual(
            document[begin:end], contracts.markdown_table(),
            "regenerate the table with `uv run --no-project scripts/session_pty.py --describe`",
        )

    def test_describe_prints_the_table_without_creating_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = run_driver("--describe", env={**os.environ, "TMPDIR": temporary})
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(result.stdout.decode(), contracts.markdown_table())
            self.assertEqual(list(Path(temporary).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
