"""Real CLI qualification of the reproducible operational-alpha demo."""

from __future__ import annotations

import argparse
import json
import sqlite3
import subprocess
import sys
import tempfile
from contextlib import closing
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from create_alpha_demo import create  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("wheel", type=Path)
    parser.add_argument("wheelhouse", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cli, wheel, wheelhouse = (
        path.resolve(strict=True) for path in (args.cli, args.wheel, args.wheelhouse)
    )
    cases = []
    with tempfile.TemporaryDirectory(prefix="tf-alpha-") as temporary:
        root = Path(temporary).resolve() / "workspace"
        receipt = create(root, cli, wheel, wheelhouse)
        assert receipt["datasets"] == 5 and len(receipt["schedules"]) == 2
        assert receipt["coordinator"] == "stopped"
        cases.append("fresh CLI setup registers five Polars/SQL outputs and creates cached history")
        cases.append(
            "fixed snapshots with registered identities seed two paused schedules and a view"
        )
        before = (root / "workspace.toml").read_bytes()
        try:
            create(root, cli, wheel, wheelhouse)
        except ValueError:
            pass
        else:
            raise AssertionError("Demo creation overwrote an existing workspace")
        assert (root / "workspace.toml").read_bytes() == before
        cases.append("repeated setup refuses existing workspace without changing its configuration")

        def heads() -> list[tuple[object, ...]]:
            with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as database:
                return database.execute("SELECT * FROM dataset_heads ORDER BY 1,2").fetchall()

        original_heads = heads()
        assert len(original_heads) == 6
        cases.append("review has one head while master retains the diamond and SQL summary")
        source = root / "src/orders.py"
        original = source.read_text()
        source.write_text(original.replace("150 + (n * 17) % 850", "-1 if n == 2 else 150"))
        assert source.read_text() != original
        try:
            failed = subprocess.run(
                [
                    cli,
                    "--workspace",
                    root,
                    "build",
                    "analytics/region_summary",
                    "--mode",
                    "full",
                    "--branch",
                    "master",
                    "--force",
                    "--json",
                ],
                capture_output=True,
                text=True,
                timeout=180,
                check=False,
            )
            report = json.loads(failed.stdout)["result"]["data"]
            assert failed.returncode == 1 and report["state"] == "FAILED", report
            assert heads() == original_heads
            assert any(job["state"] == "BLOCKED" for job in report["jobs"]), report
            cases.append(
                "output FAIL on forced source preserves every last-good head and blocks descendants"
            )
        finally:
            source.write_text(original)
        restored = subprocess.run(
            [
                cli,
                "--workspace",
                root,
                "build",
                "analytics/region_summary",
                "--mode",
                "full",
                "--branch",
                "master",
                "--force",
                "--json",
            ],
            capture_output=True,
            text=True,
            timeout=180,
            check=True,
        )
        assert json.loads(restored.stdout)["result"]["data"]["state"] == "SUCCEEDED"
        cases.append("restoring source permits a real successful five-job Polars/SQL build")
    args.output.write_text(json.dumps({"passed": len(cases), "cases": cases}, indent=2) + "\n")


if __name__ == "__main__":
    main()
