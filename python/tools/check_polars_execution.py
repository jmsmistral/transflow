"""Native T058 qualification against a built wheel, real supervisor and Rust artifact service."""

from __future__ import annotations

import hashlib
import importlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any
from uuid import uuid4

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow.declarations import get_declaration  # noqa: E402
from transflow_worker.canonical import catalog_fingerprint  # noqa: E402
from transflow_worker.discovery import _declaration  # noqa: E402
from transflow_worker.source_index import ModuleIndex  # noqa: E402


def command(args: list[str], *, ok: bool = True) -> dict[str, Any]:
    result = subprocess.run(args, capture_output=True, text=True, timeout=180, check=False)
    if ok:
        assert result.returncode == 0, result.stderr + result.stdout
        value: dict[str, Any] = json.loads(result.stdout)
        return value
    assert result.returncode != 0
    return {"error": result.stderr}


def main() -> None:
    probe = str(Path(sys.argv[1]).resolve(strict=True))
    normalization = str(Path(sys.argv[2]).resolve(strict=True))
    wheel = Path(sys.argv[3]).resolve(strict=True)
    pl = importlib.import_module("polars")
    checks: list[str] = []
    with tempfile.TemporaryDirectory(prefix="tf-polars-") as temporary:
        root = Path(temporary).resolve()
        launcher = root / "worker"
        launcher.write_text(
            f"#!{sys.executable} -I\nimport sys\nsys.path.insert(0, {str(wheel)!r})\n"
            "from transflow_worker.cli import main\n"
            "raise SystemExit(main(sys.argv[sys.argv.index('transflow_worker')+1:]))\n"
        )
        launcher.chmod(0o700)

        def case(
            name: str,
            body: str,
            *,
            expected: bool = True,
            precision: bool = False,
            mutate: str | None = None,
            inputs: str = "items=Input('raw/items')",
            schema: object = None,
            params: str = "",
            large: bool = False,
        ) -> dict[str, Any]:
            directory = root / name
            directory.mkdir()
            source = directory / "input"
            source.mkdir()
            workspace = directory / "workspace"
            paths = ["part[2].parquet", "part[1].parquet"]
            baseline: dict[str, Any] | None = None
            if precision:
                baseline = command([normalization, "generate", str(directory / "precision")])
                source = directory / "precision"
                paths = ["precision.parquet"]
            elif large:
                pa = importlib.import_module("pyarrow")
                pq = importlib.import_module("pyarrow.parquet")
                paths = ["large.parquet"]
                with pq.ParquetWriter(source / paths[0], pa.schema([("id", pa.int64())])) as writer:
                    for start in range(0, 8_000_000, 16_000):
                        writer.write_table(
                            pa.table(
                                {"id": pa.array(range(start, start + 16_000), type=pa.int64())}
                            )
                        )
            else:
                for path, value in zip(paths, (199, 0), strict=True):
                    pl.DataFrame({"id": [value]}).write_parquet(source / path)
                pl.DataFrame({"id": [999]}).write_parquet(source / "not-in-manifest.parquet")
            staged = command([probe, "stage", str(workspace), str(source), *paths])
            capture = directory / "capture"
            (capture / "src").mkdir(parents=True)
            output = directory / "output"
            output.mkdir(mode=0o700)
            counter = directory / "counter"
            text = (
                "import polars as pl\nfrom transflow import transform, Output, Input, Parameter\n"
                "from pathlib import Path\n"
                f"@transform(output=Output('curated/items', schema={schema!r}), {inputs}{params})\n"
                + body.replace("COUNTER_PATH", repr(str(counter)))
            )
            file = capture / "src/producer.py"
            file.write_text(text)
            namespace: dict[str, Any] = {"__name__": "producer", "__file__": str(file)}
            exec(compile(text, str(file), "exec"), namespace)
            declaration = get_declaration(namespace["produce"])
            assert declaration is not None
            index = ModuleIndex.build(("src",), ("src/producer.py",))
            entry = next(e for e in index.modules if e.name == "producer")
            producer = json.loads(json.dumps(_declaration(declaration, entry)))
            catalog = {
                "format_version": 1,
                "workspace_id": str(uuid4()),
                "source_snapshot_id": str(uuid4()),
                "catalog_fingerprint": "0" * 64,
                "entries": [],
            }
            catalog["catalog_fingerprint"] = catalog_fingerprint(catalog).hex
            bindings = [
                {
                    "alias": binding["alias"],
                    "dataset": {
                        "workspace_id": catalog["workspace_id"],
                        "dataset_id": str(uuid4()),
                    },
                    "version_id": str(uuid4()),
                    **staged,
                }
                for binding in producer["inputs"]
            ]
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
                        "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
                        "byte_length": str(file.stat().st_size),
                    }
                ],
                "catalog": catalog,
                "environment_fingerprint": "a" * 64,
                "result_directory": str(output),
                "producer": producer,
                "inputs": bindings,
                "compression": "zstd",
                "row_group_size": "8192",
                "threads": "2",
                "context": {
                    "build_id": str(uuid4()),
                    "job_id": str(uuid4()),
                    "evaluation_time": {
                        "type": "timestamp",
                        "value": "2026-09-21T00:00:00.000000Z",
                        "unit": "us",
                        "timezone": "UTC",
                    },
                    "random_seed": "42",
                    "parameters": [
                        {"name": p["name"], "value": p["default"]} for p in producer["parameters"]
                    ],
                },
            }
            if mutate == "source":
                file.write_text(text + "\nchanged = True\n")
            if mutate == "alias":
                bindings[0]["alias"] = "missing"
            if mutate == "digest":
                bindings[0]["manifest"]["files"][0]["sha256"] = "f" * 64
            if mutate == "existing":
                (output / "part-00000.parquet").write_bytes(b"preserve")
            request_path = directory / "request.json"
            request_path.write_text(json.dumps(request))
            result = command([probe, "execute", str(workspace), str(launcher), str(request_path)])
            assert result["execution"]["ok"] is expected, (name, result)
            assert result["head_generation"] == result["versions"] == result["events"] == 1
            if expected:
                assert result["worker"]["logs_complete"] and result["worker"]["threads"] == 2
                if not large:
                    inspected = command(
                        [normalization, "inspect", str(output / "part-00000.parquet")]
                    )
                    result["inspected"] = inspected
                if precision:
                    assert result["inspected"] == baseline, (name, result["inspected"], baseline)
            if mutate == "existing":
                assert (output / "part-00000.parquet").read_bytes() == b"preserve"
            result["counter"] = counter.read_text() if counter.exists() else ""
            checks.append(name)
            return result

        exact = case("exact-files", "def produce(items):\n    return items\n")
        assert [r[0]["value"] for r in exact["inspected"]["rows"]] == ["199", "0"]
        case("dataframe", "def produce(items):\n    return pl.DataFrame({'id': [0, 199]})\n")
        empty = case("empty", "def produce(items):\n    return items.filter(pl.lit(False))\n")
        assert empty["inspected"]["rows"] == []
        case("precision", "def produce(items):\n    return items\n", precision=True)
        once = case(
            "once",
            """def produce(items):
    with Path(COUNTER_PATH).open('a') as log: log.write('call\\n')
    def batch(frame):
        with Path(COUNTER_PATH).open('a') as log: log.write('materialize\\n')
        return frame
    result = items.map_batches(batch, schema=items.collect_schema(), streamable=False)
    def forbidden(*args, **kwargs): raise AssertionError('unconditional collect')
    result.collect = forbidden
    return result
""",
        )
        assert once["counter"].splitlines() == ["call", "materialize"], once
        alias = case(
            "aliases-and-validation-role",
            """def produce(left, right, *, ctx):
    assert ctx.random_seed == 42
    assert ctx.evaluation_time.year == 2026
    ctx.log('context log')
    return left.join(right, on='id')
""",
            inputs=(
                "left=Input('raw/items'), right=Input('raw/items'), "
                "audit=Input('raw/items', role='validation')"
            ),
        )
        assert len(alias["inspected"]["rows"]) == 2
        case(
            "threads",
            """def produce(items):
    assert pl.thread_pool_size() == 2
    import os
    for name in ['POLARS_MAX_THREADS', 'OMP_NUM_THREADS', 'OPENBLAS_NUM_THREADS',
                 'MKL_NUM_THREADS', 'NUMEXPR_MAX_THREADS', 'VECLIB_MAXIMUM_THREADS']:
        assert os.environ[name] == '2'
    return items
""",
        )
        case(
            "parameter",
            """def produce(items, *, ctx):
    assert ctx.params['n'] == 7
    try: ctx.params['n'] = 3
    except TypeError: pass
    else: raise AssertionError('mutable params')
    return items
""",
            params=", params={'n': Parameter({'type': 'i64'}, {'type': 'i64', 'value': '7'})}",
        )
        case("source", "def produce():\n    return pl.DataFrame({'id': [1]})\n", inputs="")
        for name, body in [
            ("none", "None"),
            ("multiple", "[items, items]"),
            ("generator", "(x for x in range(2))"),
            ("object", "pl.DataFrame({'bad': pl.Series([object()], dtype=pl.Object)})"),
            ("null-type", "pl.DataFrame({'bad': [None]})"),
            ("duration", "pl.DataFrame({'bad': pl.Series([1], dtype=pl.Duration('ns'))})"),
        ]:
            case(name, f"def produce(items):\n    return {body}\n", expected=False)
        wrong = {
            "format_version": 1,
            "fields": [{"name": "id", "logical_type": {"type": "i32"}, "nullable": True}],
        }
        case(
            "schema-mismatch",
            "def produce(items):\n    return items\n",
            expected=False,
            schema=wrong,
        )
        for mutation in ["source", "alias", "digest", "existing"]:
            case(
                "changed-" + mutation,
                "def produce(items):\n    return items\n",
                expected=False,
                mutate=mutation,
            )
        case(
            "exception",
            "def produce(items):\n    raise RuntimeError('synthetic')\n",
            expected=False,
        )
        if "--quick" not in sys.argv:
            large = case(
                "streaming",
                """def produce(items):
    import atexit, resource, sys, json
    def record():
        peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        rss = peak if sys.platform == 'darwin' else peak * 1024
        print(json.dumps({'peak_rss_bytes': rss}), file=sys.stderr)
    atexit.register(record)
    return items.select([(pl.col('id') + n).alias('c' + str(n)) for n in range(16)])
""",
                large=True,
            )
            rss = json.loads(large["worker"]["stderr"].strip())["peak_rss_bytes"]
            assert large["execution"]["manifest"]["files"][0]["row_count"] == "8000000"
            assert rss < 8_000_000 * 16 * 8, ("output must exceed measured worker peak memory", rss)
        else:
            rss = None
    print(
        json.dumps(
            {
                "result": "pass",
                "checks": checks,
                "polars": pl.__version__,
                "streaming_peak_rss_bytes": rss,
                "streaming_logical_bytes": 8_000_000 * 16 * 8 if rss else None,
            }
        )
    )


if __name__ == "__main__":
    main()
