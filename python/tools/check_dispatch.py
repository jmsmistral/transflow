"""Real accepted-plan dispatch against managed, hash-locked packaged workers."""

from __future__ import annotations

import json
import sqlite3
import subprocess
import sys
import tempfile
import time
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow_worker.environment import lock_environment, sync_environment  # noqa: E402


def main() -> None:
    probe, cli, wheel, wheelhouse = [Path(v).resolve(strict=True) for v in sys.argv[1:5]]
    cases: list[str] = []
    with tempfile.TemporaryDirectory(prefix="tf-dispatch-") as temporary:
        root = Path(temporary).resolve() / "workspace"
        subprocess.run([cli, "init", root], check=True, capture_output=True)
        (root / "requirements.in").write_text("polars==1.44.2\nduckdb==1.5.5\npyarrow==25.0.1\n")
        lock_environment(
            root,
            "requirements.in",
            "requirements.lock",
            "3.14",
            wheelhouse=wheelhouse,
            offline=True,
        )
        sync_environment(
            root,
            "requirements.in",
            "requirements.lock",
            "3.14",
            wheel,
            "0.0.0.dev0",
            wheelhouse=wheelhouse,
            offline=True,
        )
        with (root / "workspace.toml").open("a") as f:
            f.write("\n[execution]\nmax_jobs=2\ncpu_tokens=2\n")
        code = """import polars as pl
from transflow import transform, Input, Output, Check
from transflow import expectations as E
@transform(output=Output("raw/items"))
def items():
    return pl.DataFrame({"id": [1, 2, 3]}).lazy()
@transform(rows=Input("raw/items"), output=Output("curated/left"))
def left(rows):
    return rows.with_columns((pl.col("id") * 2).alias("left"))
@transform(rows=Input("raw/items"), output=Output("curated/right"))
def right(rows):
    return rows.with_columns((pl.col("id") * 3).alias("right"))
@transform(left=Input("curated/left"), right=Input("curated/right"),
           output=Output("curated/end", checks=[Check(E.col("id").gte(0), "positive")]))
def end(left, right):
    return left.join(right, on="id")
"""
        header, *definitions = code.split("@transform")
        for name, body in zip(["items", "left", "right", "end"], definitions, strict=True):
            (root / f"src/{name}.py").write_text(header + "@transform" + body)

        def run(
            mode: str = "full", target: str = "curated/end", ok: bool = True, **settings: Any
        ) -> dict[str, Any]:
            p = subprocess.run(
                [probe, root, sys.executable, mode, target, json.dumps(settings)],
                capture_output=True,
                text=True,
                timeout=180,
                check=True,
            )
            r: dict[str, Any] = json.loads(p.stdout)
            assert r["ok"] is ok, (r, p.stderr)
            return r["result"] if ok else r

        def rows(sql: str, *args: object) -> list[tuple[Any, ...]]:
            with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as db:
                return db.execute(sql, args).fetchall()

        first = run()
        assert first["state"] == "Succeeded", (
            first,
            rows("SELECT state,process_json FROM attempts"),
        )
        assert [j["state"] for j in first["jobs"]] == ["SUCCEEDED"] * 4
        assert rows("SELECT count(*) FROM dataset_versions") == [(4,)]
        assert rows("SELECT count(*) FROM job_inputs") == [(4,)]
        assert rows("SELECT count(*) FROM version_inputs") == [(4,)]
        assert rows("SELECT count(*) FROM write_reservations") == [(0,)]
        assert rows("SELECT DISTINCT phase FROM phase_intervals ORDER BY phase") == [
            (s,)
            for s in sorted(
                [
                    "STARTING",
                    "VALIDATING_INPUTS",
                    "RUNNING",
                    "MATERIALIZING",
                    "VALIDATING_OUTPUTS",
                    "COMMITTING",
                ]
            )
        ]
        assert rows(
            "SELECT count(*) FROM attempts a JOIN phase_intervals p ON p.attempt_id=a.id "
            "WHERE p.phase='COMMITTING' AND (a.finished_at_us<p.started_at_us "
            "OR a.finished_at_us>p.finished_at_us)"
        ) == [(0,)]
        cases.append("full diamond publishes exact parent versions and complete evidence")
        second = run()
        assert second["state"] == "Succeeded" and all(
            j["state"] == "CACHED" and not j["attempts"] for j in second["jobs"]
        ), second
        cases.append("whole-job reuse creates no attempts")
        forced = run(force=True)
        assert all(j["state"] == "SUCCEEDED" for j in forced["jobs"]), forced
        assert rows("SELECT count(*) FROM dataset_versions") == [(8,)]
        cases.append("force executes despite identical bytes")

        def dataset(path: str) -> str:
            registry = tomllib.loads((root / ".transflow/catalog.toml").read_text())
            return str(next(d["id"] for d in registry["datasets"] if d["path"] == path))

        def head(path: str) -> str:
            return str(
                rows(
                    "SELECT h.version_id FROM dataset_heads h "
                    "JOIN data_branches b ON b.id=h.branch_id "
                    "WHERE h.dataset_id=? AND b.name='feature'",
                    dataset(path),
                )[0][0]
            )

        old_items = str(
            rows(
                "SELECT id FROM dataset_versions WHERE dataset_id=? "
                "ORDER BY published_at_us LIMIT 1",
                dataset("raw/items"),
            )[0][0]
        )
        selected = run(
            "selected", "curated/left", force=True, pins=[f"curated/left#rows={old_items}"]
        )
        assert len(selected["jobs"]) == 1 and selected["state"] == "Succeeded"
        assert rows(
            "SELECT version_id FROM job_inputs WHERE job_id=?", selected["jobs"][0]["job"]
        ) == [(old_items,)]
        cases.append("selected scope binds explicit historical pin exactly")
        before_items = head("raw/items")
        between = run("between", boundaries=["raw/items"], force=True)
        assert len(between["jobs"]) == 3 and between["state"] == "Succeeded", between
        assert head("raw/items") == before_items
        assert rows("SELECT count(*) FROM read_leases WHERE kind='build' AND released=0") == [(0,)]
        cases.append("between scope publishes only descendants and releases boundary leases")
        run("selected", "curated/end", branch="empty", ok=False)
        assert rows("SELECT count(*) FROM data_branches WHERE name='empty'") == [(0,)]
        cases.append("unresolved boundary refuses before any producer or branch creation")
        before_attempts = rows("SELECT count(*) FROM attempts")
        (root / "src/broken.py").write_text(
            "from transflow import transform, Input, Output\n"
            "@transform(x=Input('missing/data'), output=Output('bad/data'))\n"
            "def bad(x): return x\n"
        )
        run(ok=False)
        assert rows("SELECT count(*) FROM attempts") == before_attempts
        (root / "src/broken.py").unlink()
        cases.append("invalid complete graph starts no producer")
        original_items = (root / "src/items.py").read_text()
        frozen = run(
            "selected",
            "raw/items",
            force=True,
            replace_source=original_items.replace("[1, 2, 3]", "[999]"),
        )
        assert frozen["state"] == "Succeeded"
        assert rows(
            "SELECT a.row_count FROM artifacts a "
            "JOIN dataset_versions v ON v.artifact_digest=a.digest "
            "WHERE v.id=?",
            head("raw/items"),
        ) == [(3,)]
        (root / "src/items.py").write_text(original_items)
        cases.append("accepted capture executes despite later checkout edits")
        canceled = run(cancel=True, force=True)
        assert canceled["state"] == "Canceled" and all(
            j["state"] == "CANCELED" and not j["attempts"] for j in canceled["jobs"]
        ), canceled
        cases.append("pre-dispatch cancellation starts no attempts and cleans reservations")

        original_left = (root / "src/left.py").read_text()
        old_left = head("curated/left")
        (root / "src/left.py").write_text(
            original_left.replace(
                'Input("raw/items")',
                'Input("raw/items", checks=[Check(E.col("id").gte(99), "input_fail")])',
            )
        )
        failed = run("selected", "curated/left", force=True)
        assert failed["state"] == "Failed" and head("curated/left") == old_left, failed
        process = json.loads(
            rows("SELECT process_json FROM attempts WHERE job_id=?", failed["jobs"][0]["job"])[0][0]
        )
        assert process["transform"] is None and process["checks"]
        assert rows(
            "SELECT c.outcome FROM check_results c "
            "JOIN attempts a ON a.id=c.attempt_id WHERE a.job_id=?",
            failed["jobs"][0]["job"],
        ) == [("VIOLATION",)]
        cases.append("input FAIL retains evidence and prevents producer invocation")
        (root / "src/left.py").write_text(
            original_left.replace(
                'Output("curated/left")',
                'Output("curated/left", checks=[Check(E.col("id").gte(99), "output_fail")])',
            )
        )
        failed = run(force=True)
        assert failed["state"] == "Failed" and head("curated/left") == old_left, failed
        assert any(
            j["state"] == "BLOCKED" and j["dataset"] == dataset("curated/end")
            for j in failed["jobs"]
        )
        assert rows("SELECT count(*) FROM failed_check_candidates")[0][0] >= 1
        cases.append(
            "output FAIL retains invisible bytes and blocks planned descendants without fallback"
        )
        (root / "src/left.py").write_text(
            original_left.replace(
                'Output("curated/left")',
                'Output("curated/left", checks=[Check(E.col("id").gte(99), '
                '"warn", on_error="WARN")])',
            )
        )
        warned = run("selected", "curated/left", force=True)
        assert warned["state"] == "Succeeded" and head("curated/left") != old_left
        cases.append("exact WARN violation permits publication")
        (root / "src/left.py").write_text(original_left)

        # Both ready siblings must run simultaneously to pass this bounded barrier.
        originals = {name: (root / f"src/{name}.py").read_text() for name in ("left", "right")}
        for name, other in (("left", "right"), ("right", "left")):
            body = originals[name].replace(
                "    return rows",
                "    from pathlib import Path\n    import time\n"
                f"    Path({str(root / name)!r}).touch()\n"
                "    deadline = time.monotonic() + 20\n"
                f"    while not Path({str(root / other)!r}).exists():\n"
                "        if time.monotonic() > deadline: "
                "raise RuntimeError('sibling did not start')\n"
                "        time.sleep(0.01)\n    return rows",
            )
            (root / f"src/{name}.py").write_text(body)
        parallel = run(force=True)
        assert parallel["state"] == "Succeeded", parallel
        cases.append("independent ready siblings overlap under two job reservations")
        for name, body in originals.items():
            (root / f"src/{name}.py").write_text(body)

        # A lazy map runs only in the sink: observe persisted MATERIALIZING before canceling.
        marker = root / "inside_sink"
        wait = f"""def slow(batch):
    from pathlib import Path
    import time
    Path({str(marker)!r}).touch()
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline: time.sleep(0.01)
    return batch
"""
        (root / "src/left.py").write_text(
            original_left.replace(
                'return rows.with_columns((pl.col("id") * 2).alias("left"))',
                'return rows.map_batches(slow, schema={"id": pl.Int64})',
            )
            + wait
        )
        old_left = head("curated/left")
        process_worker = subprocess.Popen(
            [probe, root, sys.executable, "selected", "curated/left", '{"force":true}'],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.monotonic() + 90
            while not marker.exists():
                assert process_worker.poll() is None, process_worker.communicate()
                assert time.monotonic() < deadline, "sink did not start"
                time.sleep(0.02)
            while rows(
                "SELECT state FROM jobs WHERE build_id=(SELECT id FROM builds "
                "ORDER BY created_at_us DESC LIMIT 1)"
            ) != [("MATERIALIZING",)]:
                assert process_worker.poll() is None, process_worker.communicate()
                assert time.monotonic() < deadline, "materialization phase was not persisted"
                time.sleep(0.01)
            with closing(sqlite3.connect(root / ".transflow/runtime/catalog.sqlite")) as db:
                db.execute("UPDATE builds SET cancel_requested=1 WHERE state='RUNNING'")
                db.commit()
            output, errors = process_worker.communicate(timeout=20)
            stopped = json.loads(output)
            assert (
                process_worker.returncode == 0
                and stopped["ok"]
                and stopped["result"]["state"] == "Canceled"
            ), (stopped, errors)
        finally:
            if process_worker.poll() is None:
                process_worker.kill()
                process_worker.communicate()
        assert head("curated/left") == old_left
        assert rows("SELECT count(*) FROM write_reservations") == [(0,)]
        assert rows("SELECT count(*) FROM read_leases WHERE kind='build' AND released=0") == [(0,)]
        assert rows("SELECT count(*) FROM phase_intervals WHERE finished_at_us IS NULL") == [(0,)]
        cases.append("live sink phase and durable cancellation preserve head and release leases")
        (root / "src/left.py").write_text(original_left)
        config = root / "workspace.toml"
        original_config = config.read_text()
        config.write_text(
            original_config.replace("cpu_tokens=2", "cpu_tokens=2\nmemory_budget_mib=128")
        )
        refusal = run("selected", "raw/items", force=True, ok=False)
        assert "per-job estimate" in refusal["error"], refusal
        assert rows("SELECT count(*) FROM write_reservations") == [(0,)]
        estimated = run("selected", "raw/items", force=True, estimate=64 * 1024 * 1024)
        assert estimated["state"] == "Succeeded"
        config.write_text(original_config)
        cases.append("explicit memory estimates admit jobs and missing estimates cleanly refuse")
        (root / "src/refreshed.py").write_text(
            original_items.replace("transform, Input", "source_transform, transform, Input")
            .replace("@transform(", "@source_transform(")
            .replace("raw/items", "raw/refreshed")
        )
        for name, body in originals.items():
            (root / f"src/{name}.py").write_text(body.replace("raw/items", "raw/refreshed"))
        refreshed = run()
        refreshed_again = run()
        assert refreshed["state"] == refreshed_again["state"] == "Succeeded"
        assert all(j["state"] == "SUCCEEDED" for j in refreshed_again["jobs"])
        cases.append("always-refresh source runs again and children bind its new version")
        assert rows(
            "SELECT count(*) FROM job_inputs i JOIN jobs c ON c.id=i.job_id "
            "JOIN dataset_versions v ON v.id=i.version_id JOIN attempts a ON a.id=v.attempt_id "
            "JOIN jobs p ON p.id=a.job_id WHERE c.build_id=? AND p.build_id!=c.build_id",
            refreshed_again["build"],
        ) == [(0,)]
        cases.append("every planned binding resolves from this build despite older retained heads")
        (root / "src/refreshed.py").unlink()
        for name, body in originals.items():
            (root / f"src/{name}.py").write_text(body)
        (root / "src/items.py").write_text(
            original_items.replace("def items():", "def items(*, ctx):")
        )
        contextual = run("selected", "raw/items")
        contextual_again = run("selected", "raw/items")
        assert contextual["state"] == contextual_again["state"] == "Succeeded"
        assert contextual_again["jobs"][0]["state"] == "SUCCEEDED"
        cases.append("captured context reference conservatively keys the frozen evaluation clock")
        print(json.dumps({"cases": cases, "passed": len(cases)}))


if __name__ == "__main__":
    main()
