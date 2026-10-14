"""Regressions for the PTY driver's output-directory CLI boundary."""

from pathlib import Path
import os
import stat
import subprocess
import sys
import tempfile
import unittest


DRIVER = Path(__file__).with_name("session_pty.py")


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


if __name__ == "__main__":
    unittest.main()
