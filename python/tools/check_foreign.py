"""Real two-workspace provider/replica/build/replay acceptance (T070–T072)."""

from __future__ import annotations

import argparse
import json
import selectors
import signal
import sqlite3
import subprocess
import sys
import tempfile
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "python/sdk/src"), str(REPO / "python/worker/src")]
from transflow_worker.environment import lock_environment, sync_environment  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("cli", type=Path)
    parser.add_argument("wheel", type=Path)
    parser.add_argument("wheelhouse", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    cli, wheel, wheelhouse = (
        p.resolve(strict=True) for p in (args.cli, args.wheel, args.wheelhouse)
    )
    report: dict[str, Any] = {"tasks": ["T070", "T071", "T072"], "cases": [], "commands": []}

    def passed(label: str) -> None:
        report["cases"].append(label)
        print(label, file=sys.stderr, flush=True)

    with tempfile.TemporaryDirectory(prefix="tf-foreign-") as temp:
        root = Path(temp).resolve()
        provider, consumer = root / "provider", root / "consumer"

        def run(workspace: Path, *command: str, ok: bool = True) -> dict[str, Any]:
            p = subprocess.run(
                [cli, "--workspace", workspace, *command, "--json"],
                text=True,
                capture_output=True,
                timeout=180,
                check=False,
            )
            report["commands"].append(
                {
                    "workspace": workspace.name,
                    "args": list(command),
                    "exit": p.returncode,
                    "stdout": p.stdout,
                    "stderr": p.stderr,
                }
            )
            assert (p.returncode == 0) is ok, (command, p.stdout, p.stderr)
            value: dict[str, Any] = json.loads(p.stdout)
            return value

        def rows(workspace: Path, sql: str, *values: object) -> list[tuple[Any, ...]]:
            database = workspace / ".transflow/runtime/catalog.sqlite"
            with closing(sqlite3.connect(database.as_uri() + "?mode=rw", uri=True)) as db:
                db.execute("PRAGMA query_only=ON")
                return db.execute(sql, values).fetchall()

        def build(workspace: Path, *command: str, ok: bool = True) -> dict[str, Any]:
            result = run(workspace, "build", *command, "--python", sys.executable, ok=ok)
            return (result.get("result") or {}).get("data", {})  # type: ignore[no-any-return]

        for workspace in (provider, consumer):
            run(workspace, "init", str(workspace))
            (workspace / "requirements.in").write_text(
                "polars==1.44.2\nduckdb==1.5.5\npyarrow==25.0.1\n"
            )
            lock_environment(
                workspace,
                "requirements.in",
                "requirements.lock",
                "3.14",
                wheelhouse=wheelhouse,
                offline=True,
            )
            sync_environment(
                workspace,
                "requirements.in",
                "requirements.lock",
                "3.14",
                wheel,
                "0.0.0.dev0",
                wheelhouse=wheelhouse,
                offline=True,
            )
        (provider / "src/raw.py").write_text(
            "import polars as pl\nfrom transflow import transform, Output\n"
            '@transform(output=Output("raw/rows"))\n'
            'def rows(): return pl.DataFrame({"id":[1,2,3]}).lazy()\n'
        )
        (provider / "src/base.py").write_text(
            "from transflow import transform, Input, Output, Check\n"
            "from transflow import expectations as E\n"
            '@transform(x=Input("raw/rows"), output=Output("curated/base", '
            'checks=[Check(E.col("id").gte(0), "positive")]))\ndef base(x): return x\n'
        )
        original_provider = build(provider, "curated/base", "--branch", "master")
        assert original_provider["state"] == "SUCCEEDED", original_provider
        dataset = next(
            d["id"]
            for d in tomllib.loads((provider / ".transflow/catalog.toml").read_text())["datasets"]
            if d["path"] == "curated/base"
        )
        version, artifact = rows(
            provider, "SELECT id,artifact_digest FROM dataset_versions WHERE dataset_id=?", dataset
        )[0]
        provider_id = tomllib.loads((provider / "workspace.toml").read_text())["workspace_id"]
        run(
            consumer,
            "external",
            "add",
            "--workspace",
            str(provider),
            "--dataset",
            "curated/base",
            "--as",
            "market.base",
        )
        definition = consumer / "src/result.py"
        header = (
            "from transflow import transform, Input, Output, Check, Branch\n"
            "from transflow import expectations as E\nfrom transflow.catalog import C\n"
        )

        def source(selector: str = "", check: str = 'E.col("id").gte(0)') -> None:
            definition.write_text(
                header + f"@transform(x=Input(C.external.market.base{selector}, "
                f'checks=[Check({check}, "consumer-check")]), '
                'output=Output("curated/result"))\ndef result(x): return x\n'
            )

        source()
        provider_source = (provider / "src/base.py").read_text()
        # Even import-time side effects must never run during provider resolution.
        (provider / "src/base.py").write_text(
            'raise RuntimeError("PROVIDER_MUST_NOT_BE_IMPORTED")\n'
        )
        provider_attempts = rows(provider, "SELECT count(*) FROM attempts")
        first = build(consumer, "curated/result", "--branch", "feature", "--force")
        assert first["state"] == "SUCCEEDED", first
        assert len(first["jobs"]) == 1
        assert rows(provider, "SELECT count(*) FROM attempts") == provider_attempts
        assert rows(
            consumer,
            "SELECT workspace_id,dataset_id,version_id,artifact_digest,copy_state FROM replicas",
        ) == [(provider_id, dataset, version, artifact, "VERIFIED")]
        assert rows(consumer, "SELECT starting_branch,resolved_branch FROM version_inputs") == [
            ("master", "master")
        ]
        assert rows(
            provider, "SELECT count(*) FROM read_leases WHERE kind='copy' AND released=0"
        ) == [(0,)]
        local_file = (
            consumer / ".transflow/runtime/objects" / artifact[:2] / artifact / "part-00000.parquet"
        )
        provider_file = (
            provider / ".transflow/runtime/objects" / artifact[:2] / artifact / "part-00000.parquet"
        )
        assert local_file.read_bytes() == provider_file.read_bytes()
        assert local_file.stat().st_ino != provider_file.stat().st_ino
        passed("full_force_copies_verified_bytes_preserves_origin_and_never_imports_provider")
        plan = run(
            consumer, "plan", "curated/result", "--branch", "feature", "--python", sys.executable
        )["result"]
        assert plan["reads"][0]["origin_workspace"] == provider_id
        assert plan["reads"][0]["path"] == "external/market/base"
        passed("plan_reports_foreign_origin_and_alias")
        expanded = run(
            consumer,
            "upstream",
            "curated/result",
            "--branch",
            "feature",
            "--expand-external",
            "--python",
            sys.executable,
        )["result"]
        assert expanded["external_expanded"]
        assert len(expanded["foreign_provenance"]["nodes"]) == 2
        assert all(n["executable"] is False for n in expanded["foreign_provenance"]["nodes"])
        assert rows(consumer, "SELECT count(*) FROM replicas") == [(1,)]
        passed("upstream_expands_exact_read_only_provenance_without_ancestor_copies")
        source(", branch=Branch.CURRENT")
        # Provider's initialized policy falls back to master; consumer --no-fallback must not leak.
        current = build(consumer, "curated/result", "--branch", "feature", "--no-fallback")
        assert current["state"] == "SUCCEEDED", current
        assert rows(
            consumer,
            "SELECT starting_branch,resolved_branch FROM version_inputs "
            "ORDER BY rowid DESC LIMIT 1",
        ) == [("feature", "master")]
        source(", branch=Branch.CURRENT, stop_branch_fallback=True")
        build(consumer, "curated/result", "--branch", "feature", ok=False)
        source(', branch="master", stop_branch_fallback=True')
        assert build(consumer, "curated/result", "--branch", "feature")["state"] == "SUCCEEDED"
        passed(
            "registered_current_named_and_strict_provider_policy_are_independent_of_consumer_override"
        )
        run(
            consumer,
            "external",
            "add",
            "--workspace",
            str(provider),
            "--dataset",
            "curated/base",
            "--as",
            "market.strict",
            "--branch",
            "missing",
            "--no-fallback",
        )
        definition.write_text(
            header + "@transform(x=Input(C.external.market.base), "
            'y=Input(C.external.market.strict, role="validation"), '
            'output=Output("curated/result"))\ndef result(x): return x\n'
        )
        build(consumer, "curated/result", "--branch", "aliases", ok=False)
        build(
            consumer,
            "curated/result",
            "--branch",
            "aliases",
            "--pin",
            f"external/market/base={version}",
            ok=False,
        )
        aliases = build(
            consumer,
            "curated/result",
            "--branch",
            "aliases",
            "--pin",
            f"curated/result#y={version}",
        )
        assert aliases["state"] == "SUCCEEDED", aliases
        bindings = aliases["jobs"][0]["inputs"]
        assert {b["alias"] for b in bindings} == {"x", "y"}
        assert all(b["version"] == version for b in bindings)
        passed("registration_override_repeated_aliases_validation_dependency_and_qualified_pins")
        source(check='E.col("id").lt(0)')
        heads = rows(consumer, "SELECT version_id FROM dataset_heads")
        failed = build(consumer, "curated/result", "--branch", "feature", ok=False)
        assert failed["state"] == "FAILED", failed
        assert rows(consumer, "SELECT version_id FROM dataset_heads") == heads
        assert rows(provider, "SELECT outcome FROM check_results") == [("PASS",)]
        passed("consumer_checks_preserve_last_good_and_provider_output_certificates")
        source()
        server = subprocess.Popen(
            [cli, "--workspace", provider, "serve"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            # Read the explicit ready notification; no arbitrary delay for ownership.
            assert server.stderr is not None
            with selectors.DefaultSelector() as ready:
                ready.register(server.stderr, selectors.EVENT_READ)
                assert ready.select(timeout=15), "provider coordinator did not become ready"
                assert "Coordinator ready" in server.stderr.readline()
            live = build(consumer, "curated/result", "--branch", "live")
            assert live["state"] == "SUCCEEDED", live
            assert server.poll() is None
            assert rows(provider, "SELECT count(*) FROM attempts") == provider_attempts
            passed("existing_provider_coordinator_serves_metadata_without_second_writer")
        finally:
            server.send_signal(signal.SIGINT)
            server.communicate(timeout=20)
        # Same locator, wrong identity must fail rather than reuse a cached latest replica.
        config = provider / "workspace.toml"
        saved = config.read_text()
        config.write_text(saved.replace(provider_id, "99999999-9999-4999-8999-999999999999"))
        build(consumer, "curated/result", "--branch", "mismatch", ok=False)
        config.write_text(saved)
        passed("mismatched_provider_uuid_is_rejected_for_fresh_resolution")
        (provider / "src/base.py").write_text(provider_source)
        assert (
            build(provider, "curated/base", "--branch", "master", "--force")["state"] == "SUCCEEDED"
        )
        (provider / "src/base.py").write_text(
            'raise RuntimeError("PROVIDER_MUST_NOT_BE_IMPORTED")\n'
        )
        latest = run(
            consumer, "plan", "curated/result", "--branch", "new-head", "--python", sys.executable
        )["result"]["reads"][0]["version"]
        assert latest != version
        assert rows(
            provider, "SELECT version_id FROM dataset_heads WHERE dataset_id=?", dataset
        ) == [(latest,)]
        assert rows(consumer, "SELECT count(*),count(DISTINCT artifact_digest) FROM replicas") == [
            (2, 1)
        ]
        passed("fresh_resolution_observes_new_head_while_equal_bytes_keep_distinct_origin_versions")
        provider.rename(root / "provider-unavailable")
        build(consumer, "curated/result", "--branch", "offline", ok=False)
        pinned = build(
            consumer,
            "curated/result",
            "--branch",
            "offline",
            "--pin",
            f"curated/result#x={version}",
        )
        assert pinned["state"] == "SUCCEEDED", pinned
        replay = run(consumer, "build", "replay", first["id"], "--branch", "replayed")["result"][
            "data"
        ]
        assert replay["state"] == "SUCCEEDED", replay
        assert rows(
            consumer,
            "SELECT DISTINCT origin_workspace_id,origin_dataset_id,origin_version_id "
            "FROM version_inputs",
        ) == [(provider_id, dataset, version)]
        passed(
            "offline_exact_pin_and_faithful_replay_survive_provider_removal_but_fresh_latest_fails"
        )
        unavailable = run(
            consumer,
            "upstream",
            "curated/result",
            "--branch",
            "offline",
            "--expand-external",
            "--python",
            sys.executable,
        )["result"]
        assert (
            unavailable["foreign_provenance"]["nodes"][0]["availability"] == "provider_unavailable"
        )
        passed("unavailable_provider_lineage_is_explicit")
        # Required retained bytes must still be strictly verified on the offline path.
        local_file.chmod(0o600)
        local_file.write_bytes(b"corrupt")
        build(
            consumer,
            "curated/result",
            "--branch",
            "corrupt",
            "--pin",
            f"curated/result#x={version}",
            ok=False,
        )
        passed("corrupt_replica_cannot_satisfy_offline_pin")
    report["result"] = "passed"
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
