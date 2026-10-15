"""Review launches must build successfully before executing the contributor CLI."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


class ReviewLauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.repo = self.root / "checkout with spaces"
        self.workspace = self.root / "workspace with spaces"
        self.workspace.mkdir()
        (self.repo / "tools").mkdir(parents=True)
        (self.repo / "web/node_modules").mkdir(parents=True)
        (self.repo / "target/debug").mkdir(parents=True)
        self.script = self.repo / "tools/serve-review.sh"
        shutil.copyfile(Path(__file__).resolve().parents[1] / "serve-review.sh", self.script)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.events = self.root / "events"
        stub = '#!/bin/sh\nprintf "%s\\n" "$0" "$@" >> "$REVIEW_EVENTS"\n'
        for name in ("cargo", "npm"):
            command = self.bin / name
            command.write_text(
                stub
                + (
                    'if [ "$4" = build ]; then exit "${REVIEW_FAIL_BUILD:-0}"; fi\n'
                    if name == "npm"
                    else ""
                )
                + f'exit "${{REVIEW_FAIL_{name.upper()}:-0}}"\n'
            )
            command.chmod(0o755)
        cli = self.repo / "target/debug/transflow"
        cli.write_text(stub)
        cli.chmod(0o755)
        self.env = {
            **os.environ,
            "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
            "REVIEW_EVENTS": str(self.events),
        }

    def run_script(self, *args, **extra_env):
        return subprocess.run(
            ["bash", str(self.script), *map(str, args)],
            cwd=self.root,
            env={**self.env, **extra_env},
            capture_output=True,
            text=True,
            check=False,
        )

    def lines(self):
        return self.events.read_text().splitlines() if self.events.exists() else []

    def test_builds_before_launch_and_preserves_paths_and_options(self):
        result = self.run_script(
            "workspace with spaces",
            "--port",
            "0",
            "--open",
            CARGO_TARGET_DIR=str(self.root / "different target"),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.lines(),
            [
                str(self.bin / "cargo"),
                "build",
                "--locked",
                "--offline",
                "-p",
                "transflow",
                "--target-dir",
                str(self.repo / "target"),
                str(self.bin / "npm"),
                "--prefix",
                "web",
                "run",
                "prepare:types",
                str(self.bin / "npm"),
                "--prefix",
                "web",
                "run",
                "build",
                str(self.repo / "target/debug/transflow"),
                "--workspace",
                str(self.workspace),
                "serve",
                "--ui-dir",
                str(self.repo / "web/dist"),
                "--port",
                "0",
                "--open",
            ],
        )

    def test_cargo_failure_does_not_launch_existing_binary(self):
        result = self.run_script(self.workspace, REVIEW_FAIL_CARGO="19")
        self.assertEqual(result.returncode, 19)
        self.assertEqual(self.lines()[0], str(self.bin / "cargo"))
        self.assertNotIn(str(self.repo / "target/debug/transflow"), self.lines())
        self.assertNotIn(str(self.bin / "npm"), self.lines())

    def test_web_failure_does_not_launch_existing_binary(self):
        result = self.run_script(self.workspace, REVIEW_FAIL_NPM="23")
        self.assertEqual(result.returncode, 23)
        self.assertNotIn(str(self.repo / "target/debug/transflow"), self.lines())

    def test_ui_build_failure_does_not_launch_existing_binary(self):
        result = self.run_script(self.workspace, REVIEW_FAIL_BUILD="29")
        self.assertEqual(result.returncode, 29)
        self.assertEqual(self.lines().count(str(self.bin / "npm")), 2)
        self.assertNotIn(str(self.repo / "target/debug/transflow"), self.lines())

    def test_missing_dependencies_do_not_install_or_launch(self):
        (self.repo / "web/node_modules").rmdir()
        result = self.run_script(self.workspace)
        self.assertEqual(result.returncode, 1)
        self.assertIn("npm --prefix web ci --ignore-scripts", result.stderr)
        self.assertEqual(self.lines(), [])

    def test_help_and_missing_workspace_do_not_build(self):
        self.assertEqual(self.run_script("--help").returncode, 0)
        self.assertEqual(self.run_script().returncode, 2)
        self.assertEqual(self.run_script(self.root / "missing").returncode, 2)
        self.assertEqual(self.lines(), [])


if __name__ == "__main__":
    unittest.main()
