"""Native T062 gates: real SQLite heads, exact Parquet, packaged Polars/DuckDB workers."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any
from uuid import UUID, uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow.declarations import get_declaration  # noqa: E402
from transflow_worker.canonical import catalog_fingerprint  # noqa: E402
from transflow_worker.discovery import _declaration  # noqa: E402
from transflow_worker.source_index import ModuleIndex  # noqa: E402


def main() -> None:
    probe, wheel = (str(Path(v).resolve(strict=True)) for v in sys.argv[1:3])
    cases: list[str] = []
    with tempfile.TemporaryDirectory(prefix="tf-lifecycle-") as temporary:
        root = Path(temporary).resolve()
        launcher = root / "worker"
        launcher.write_text(
            f"#!{sys.executable} -I\nimport sys\nsys.path.insert(0, {wheel!r})\n"
            "from transflow_worker.cli import main\n"
            "raise SystemExit(main(sys.argv[sys.argv.index('transflow_worker')+1:]))\n"
        )
        launcher.chmod(0o700)
        good = "Check(E.col('id').gte(0), 'ids').effective()"
        bad = "Check(E.col('id').gte(2), 'ids').effective()"
        warn = "Check(E.col('id').gte(2), 'ids', on_error='WARN').effective()"
        error = "Check(E.col('missing').gte(0), 'missing', on_error='WARN').effective()"

        def run(
            name: str,
            before: str = good,
            after: str = good,
            *,
            mode: str = "normal",
            ran: bool = True,
            published: bool = True,
            repeat: bool = False,
            body: str = "return items",
        ) -> dict[str, Any]:
            directory = root / name
            capture = directory / "capture"
            (capture / "src").mkdir(parents=True)
            output = directory / "output"
            output.mkdir(mode=0o700)
            counter = directory / "counter"
            inputs = f"items=Input('raw/items', checks=[{before}])"
            signature = "items"
            if repeat:
                inputs += f", other=Input('raw/items', checks=[{before}])"
                signature += ", other"
            text = (
                "from transflow import transform, Output, Input, Check\n"
                "from transflow import expectations as E\n"
                "from pathlib import Path\n"
                f"@transform(output=Output('curated/items', checks=[{after}]), {inputs})\n"
                f"def produce({signature}):\n"
                f"    with Path({str(counter)!r}).open('a') as f: f.write('x')\n"
                f"    {body}\n"
            )
            source = capture / "src/producer.py"
            source.write_text(text)
            namespace: dict[str, Any] = {"__name__": "producer", "__file__": str(source)}
            exec(compile(text, str(source), "exec"), namespace)
            declaration = get_declaration(namespace["produce"])
            assert declaration is not None
            index = ModuleIndex.build(("src",), ("src/producer.py",))
            entry = next(e for e in index.modules if e.name == "producer")
            producer = _declaration(declaration, entry)
            catalog = {
                "format_version": 1,
                "workspace_id": str(UUID(bytes=bytes([1]) * 16)),
                "source_snapshot_id": str(UUID(bytes=bytes([3]) * 16)),
                "catalog_fingerprint": "0" * 64,
                "entries": [],
            }
            catalog["catalog_fingerprint"] = catalog_fingerprint(catalog).hex
            request = {
                "format_version": 1,
                "protocol": {"major": 1, "minor": 0},
                "request_id": str(uuid4()),
                "attempt_id": str(uuid4()),
                "auth_token": "0" * 64,
                "capture_root": str(capture),
                "source_roots": ["src"],
                "files": [
                    {
                        "path": "src/producer.py",
                        "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                        "byte_length": str(source.stat().st_size),
                    }
                ],
                "catalog": catalog,
                "environment_fingerprint": "a" * 64,
                "result_directory": str(output),
                "producer": producer,
                "inputs": [],  # Probe replaces these with real retained publications.
                "compression": "zstd",
                "row_group_size": "8192",
                "threads": "1",
                "context": {
                    "build_id": str(uuid4()),
                    "job_id": str(uuid4()),
                    "evaluation_time": {
                        "type": "timestamp",
                        "value": "2026-09-23T00:00:00.000000Z",
                        "unit": "us",
                        "timezone": "UTC",
                    },
                    "random_seed": "42",
                    "parameters": [],
                },
            }
            path = directory / "request.json"
            path.write_text(json.dumps(request))
            completed = subprocess.run(
                [probe, str(directory / "workspace"), str(launcher), str(path), mode],
                capture_output=True,
                text=True,
                timeout=180,
                check=False,
            )
            assert completed.returncode == 0, (name, completed.stdout, completed.stderr)
            result: dict[str, Any] = json.loads(completed.stdout)
            assert result["outcome"]["published"] is published, (name, result)
            assert result["producer_ran"] is ran, (name, result)
            assert (counter.read_text() if counter.exists() else "") == ("x" if ran else "")
            assert result["head"] == str(UUID(bytes=bytes([20 if published else 11]) * 16))
            assert result["versions"] == (3 if published else 2), (name, result)
            assert result["provider_checks"] == 1, (name, result)
            if published:
                assert result["attempt_state"] == "SUCCEEDED"
                assert result["candidate_readable"]
                assert len(result["linked"]) == sum(len(r["checks"]) for r in result["results"])
            else:
                assert result["linked"] == []
                if ran and mode != "tamper" and body == "return items":
                    assert result["candidate_readable"], (name, result)
                if not ran and mode not in {"omitted", "wrong-version", "canceled"}:
                    assert result["attempt_state"] == "VALIDATING_INPUTS"
                    assert all(c["status"] == "SKIPPED" for c in result["skipped"])
            assert result["retained_checks"] == sum(len(r["checks"]) for r in result["results"]), (
                result
            )
            if not published and ran and mode != "tamper" and body == "return items":
                assert result["failed_candidates"] == 1, result
            else:
                assert result["failed_candidates"] == 0, result
            for r in result["results"]:
                if r["phase"] == "output":
                    assert r["artifact_digest"] == result["candidate"]
                else:
                    assert r["subject_version"] == str(UUID(bytes=bytes([10]) * 16))
            cases.append(name)
            return result

        run("pass")
        run("input-fail", before=bad, ran=False, published=False)
        run("input-warn", before=warn)
        run("input-error-warn", before=error, ran=False, published=False)
        run("output-fail", after=bad, published=False)
        warning = run("output-warn", after=warn)
        assert [r["outcome"] for r in warning["linked"]] == ["PASS", "VIOLATION"]
        run("output-error-warn", after=error, published=False)
        run("helper-error-warn", after=warn, mode="helper-error", published=False)
        repeated = run("repeated-alias", repeat=True)
        assert [r["binding"] for r in repeated["results"]] == ["items", "other", None]
        independent = run("all-input-aliases", before=bad, repeat=True, ran=False, published=False)
        assert len(independent["results"]) == 2
        run("no-checks", before="", after="")
        mixed = run("independent-output-checks", after=error + ", " + good, published=False)
        assert [c["status"] for c in mixed["results"][-1]["checks"]] == ["ERROR", "PASS"]
        run("omitted-obligation", mode="omitted", ran=False, published=False)
        run("wrong-version", mode="wrong-version", ran=False, published=False)
        run("canceled", mode="canceled", ran=False, published=False)
        run("cancel-after-materialize", mode="cancel-after-materialize", published=False)
        run("cancel-before-publish", mode="cancel-before-publish", published=False)
        run("persisted-cancel", mode="persisted-cancel", published=False)
        run("conflict", mode="conflict", published=False)
        run("tamper", mode="tamper", published=False)
        run("producer-error", body="raise ValueError('synthetic failure')", published=False)
        print(json.dumps({"result": "passed", "cases": cases}, indent=2))


if __name__ == "__main__":
    main()
