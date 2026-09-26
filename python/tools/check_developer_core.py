"""Public CLI acceptance of the synthetic customer-orders walkthrough (T068).

Requires explicitly prepared tooling, matched wheel and offline wheelhouse. Each
variant starts empty; all mutations use the CLI, all SQLite inspection is read-only.
The report includes command output even on failure and never claims G1 closure.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import tomllib
from contextlib import closing
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
FIXTURE = REPO / "examples/customer-orders"
TARGET = "curated/customer_orders"


class Journey:
    def __init__(self, root: Path, cli: Path, report: dict[str, Any], git: bool) -> None:
        self.root, self.cli, self.report = root, cli, report
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        self.env.update(PIP_NO_INDEX="1", PIP_DISABLE_PIP_VERSION_CHECK="1")
        self.env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        if not git:
            # No executable lookup, including Git; Python is always supplied explicitly.
            self.env["PATH"] = str(root.parent / "no-executables")
        self.expected = json.loads((FIXTURE / "expected.json").read_text())

    def invoke(self, *args: str, code: int = 0, human: bool = False) -> dict[str, Any]:
        command = [str(self.cli), "--workspace", str(self.root), *args]
        if not human:
            command.append("--json")
        result = subprocess.run(
            command, env=self.env, text=True, capture_output=True, timeout=240, check=False
        )
        self.report["commands"].append(
            {
                "args": list(args),
                "exit": result.returncode,
                "stdout": result.stdout,
                "stderr": result.stderr,
            }
        )
        assert result.returncode == code, (args, result.stdout, result.stderr)
        if human:
            return {}
        value: dict[str, Any] = json.loads(result.stdout)
        assert value["exit_status"] == code, value
        return value

    def build(self, *args: str, code: int = 0) -> dict[str, Any]:
        return self.invoke("build", *args, "--python", sys.executable, code=code)["result"]["data"]  # type: ignore[no-any-return]

    def rows(self, sql: str, *args: object) -> list[tuple[Any, ...]]:
        database = self.root / ".transflow/runtime/catalog.sqlite"
        # macOS's Python SQLite cannot always recreate absent WAL sidecars with
        # mode=ro after a refused command. Allow coordination files, forbid SQL
        # writes, and never create a missing database (mode=rw).
        with closing(sqlite3.connect(database.as_uri() + "?mode=rw", uri=True)) as connection:
            connection.execute("PRAGMA query_only=ON")
            return connection.execute(sql, args).fetchall()

    def registry(self) -> dict[str, str]:
        value = tomllib.loads((self.root / ".transflow/catalog.toml").read_text())
        return {d["path"]: d["id"] for d in value.get("datasets", [])}

    def heads(self, branch: str = "master") -> dict[str, str]:
        ids = {v: k for k, v in self.registry().items()}
        return {
            ids[dataset]: version
            for dataset, version in self.rows(
                "SELECT h.dataset_id,h.version_id FROM dataset_heads h "
                "JOIN data_branches b ON b.id=h.branch_id WHERE b.name=?",
                branch,
            )
        }

    def data(self, version: str) -> list[dict[str, Any]]:
        digest, manifest = self.rows(
            "SELECT a.digest,a.manifest_json FROM artifacts a "
            "JOIN dataset_versions v ON v.artifact_digest=a.digest WHERE v.id=?",
            version,
        )[0]
        files = json.loads(manifest)["files"]
        paths = [
            str(self.root / ".transflow/runtime/objects" / digest[:2] / digest / f["path"])
            for f in files
        ]
        # The qualification interpreter has the already-installed pinned Polars engine.
        result = subprocess.run(
            [
                sys.executable,
                "-I",
                "-c",
                "import json,polars as pl,sys; print(json.dumps("
                "pl.read_parquet(sys.argv[1:]).sort('order_id').to_dicts(),default=str))",
                *paths,
            ],
            text=True,
            capture_output=True,
            check=True,
            timeout=30,
        )
        return json.loads(result.stdout)  # type: ignore[no-any-return]

    def passed(self, label: str) -> None:
        self.report["cases"].append(label)
        print(f"{self.report['name']}: {label}", file=sys.stderr, flush=True)

    def job(self, build: dict[str, Any], path: str = TARGET) -> dict[str, Any]:
        return next(j for j in build["jobs"] if j["dataset"] == self.registry()[path])


def exercise(j: Journey, wheel: Path, wheelhouse: Path, git: str | None) -> None:
    root = j.root
    j.invoke("init", str(root))
    assert not (root / ".git").exists() and not j.registry()
    j.invoke(
        "env", "lock", "--python", sys.executable, "--wheelhouse", str(wheelhouse), "--no-index"
    )
    j.invoke(
        "env",
        "sync",
        "--python",
        sys.executable,
        "--wheelhouse",
        str(wheelhouse),
        "--no-index",
        "--runtime-wheel",
        str(wheel),
    )
    j.invoke("env", "check", "--python", sys.executable)
    shutil.copytree(FIXTURE / "src", root / "src", dirs_exist_ok=True)
    # Empty organization-only directory must not add a dataset or catalogue namespace.
    (root / "src/transforms/analytics").mkdir()

    def git_run(*args: str) -> str:
        assert git is not None
        return subprocess.run(
            [
                git,
                "-C",
                str(root),
                "-c",
                "user.name=Transflow Acceptance",
                "-c",
                "user.email=acceptance@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                *args,
            ],
            env=j.env,
            capture_output=True,
            text=True,
            check=True,
            timeout=30,
        ).stdout

    if git:
        git_run("init", "--template=", "-b", "master")
    registry_before = (root / ".transflow/catalog.toml").read_text()
    # First build deliberately has no validate/plan/catalog-sync ceremony.
    first = j.build(TARGET)
    assert first["state"] == "SUCCEEDED" and len(first["jobs"]) == 3, first
    assert all(job["state"] == "SUCCEEDED" for job in first["jobs"])
    assert sorted(j.registry()) == j.expected["datasets"]
    assert set(j.heads()) == set(j.registry())
    assert {job["branch"] for job in first["jobs"]} == {"master"}
    j.report["registry_before"] = registry_before
    j.report["registry_after"] = (root / ".transflow/catalog.toml").read_text()
    j.report["post_build_tooling"] = {
        "editor_overlay": (
            root / ".transflow/runtime/generated/current/transflow/catalog.pyi"
        ).is_file(),
        "browse_graph": (root / ".transflow/runtime/graphs/current").is_file(),
    }
    browse = root / ".transflow/runtime/graphs/current"
    assert j.report["post_build_tooling"]["browse_graph"]
    assert not j.report["post_build_tooling"]["editor_overlay"]
    retained = json.loads(browse.read_text())
    assert retained["source_snapshot_id"] == retained["discovery"]["source_snapshot_id"]
    captured_registry = (
        root
        / ".transflow/runtime/source-snapshots"
        / retained["source_snapshot_id"]
        / "files/.transflow/catalog.toml"
    )
    assert captured_registry.read_bytes() == (root / ".transflow/catalog.toml").read_bytes()
    assert j.invoke("catalog", "show", TARGET)["result"]["entries"][0]["producer"]["path"] == (
        "src/transforms/curated/customer_orders.py"
    )
    j.passed("first build refreshes browse metadata with registered IDs and matching source")
    registry = j.registry()
    output = j.job(first)
    observed = j.data(output["version"])
    assert observed == j.expected["rows"], observed
    checks = output["attempts"][0]["checks"]
    assert (
        sorted([c["name"], c["phase"], c["alias"], c["outcome"]] for c in checks)
        == (j.expected["checks"])
    ), checks
    assert j.rows(
        "SELECT alias,origin_version_id FROM version_inputs WHERE output_version_id=? "
        "ORDER BY alias",
        output["version"],
    ) == [(alias, j.job(first, f"raw/{alias}")["version"]) for alias in ("customers", "orders")]
    assert j.rows(
        "SELECT count(*) FROM dataset_versions WHERE source_snapshot_id=?", first["source"]
    ) == [(3,)]
    j.report["result_rows"] = observed
    j.passed(
        "fresh public build registers exactly three outputs and persists exact rows/pins/checks"
    )

    browse_before = browse.read_bytes()
    j.invoke("validate", "--python", sys.executable)
    plan = j.invoke("plan", TARGET, "--mode", "selected", "--python", sys.executable)
    assert plan["result"]["plan_id"]
    assert browse.read_bytes() == browse_before
    catalog = j.invoke("catalog", "show", TARGET)["result"]["entries"]
    assert len(catalog) == 1 and catalog[0]["dataset_id"] == registry[TARGET]
    assert catalog[0]["recent_versions"][0]["id"] == output["version"]
    upstream = j.invoke("upstream", TARGET, "--depth", "1", "--python", sys.executable)["result"]
    assert sorted(path for n in upstream["nodes"] for path in n["paths"]) == j.expected["datasets"]
    assert {e["alias"] for e in upstream["edges"]} == {"orders", "customers"}
    downstream = j.invoke("downstream", "raw/orders", "--python", sys.executable)["result"]
    assert sorted(path for n in downstream["nodes"] for path in n["paths"]) == [
        TARGET,
        "raw/orders",
    ]
    assert j.invoke("build", "show", first["id"])["result"]["data"] == first
    j.invoke("build", "list")
    j.invoke("build", "logs", first["id"], human=True)
    assert j.registry() == registry
    j.passed("public validation, plan, catalogue, lineage and execution inspection succeed")

    before = j.heads()
    cached = j.build(TARGET, "--mode", "selected")
    assert len(cached["jobs"]) == 1
    reuse = j.job(cached)
    assert reuse["state"] == "CACHED" and not reuse["attempts"]
    assert reuse["version"] == output["version"] and reuse["cached_from"]["checks"] == checks
    assert j.heads() == before
    j.passed("selected repeat reuses original version and check timestamps without source refresh")
    forced = j.build(TARGET, "--mode", "selected", "--force")
    assert len(forced["jobs"]) == 1 and j.job(forced)["state"] == "SUCCEEDED"
    assert j.job(forced)["version"] != output["version"]
    assert len(j.job(forced)["attempts"][0]["checks"]) == 3
    assert all(j.heads()[path] == before[path] for path in ("raw/customers", "raw/orders"))
    assert j.data(j.job(forced)["version"]) == observed
    j.passed("selected force executes the join and all checks while preserving source heads")
    before = j.heads()
    repeated = j.build(TARGET, "--mode", "full")
    assert len(repeated["jobs"]) == 3 and all(x["state"] == "SUCCEEDED" for x in repeated["jobs"])
    assert all(j.heads()[path] != before[path] for path in registry)
    j.passed("explicit full matches default scope and refreshes always sources")
    before = j.heads()
    bounded = j.build(TARGET, "--mode", "between", "--boundary", "raw/orders", "--force")
    assert len(bounded["jobs"]) == 1 and j.job(bounded)["state"] == "SUCCEEDED"
    assert all(j.heads()[path] == before[path] for path in ("raw/customers", "raw/orders"))
    j.passed("between force executes only after its boundary and pins the side input")

    committed = None
    if git:
        git_run(
            "add",
            "workspace.toml",
            "requirements.in",
            "requirements.lock",
            "src",
            ".transflow/catalog.toml",
            ".gitignore",
        )
        git_run("commit", "--no-gpg-sign", "-m", "Add customer-orders pipeline")
        committed = git_run("rev-parse", "HEAD").strip()
        assert ".transflow/runtime/" not in git_run("ls-files")
        git_run("switch", "-c", "feature/customer-name-cleanup")
    branch = "feature/customer-name-cleanup" if git else "experiment"
    master = j.heads()
    helper = root / "src/common/names.py"
    helper.write_text(
        helper.read_text().replace(".str.strip_chars()", ".str.strip_chars().str.to_uppercase()")
    )
    selected = j.build(TARGET, "--mode", "selected", *([] if git else ["--branch", branch]))
    feature = j.job(selected)
    assert feature["state"] == "SUCCEEDED" and feature["branch"] == branch
    assert len(selected["jobs"]) == 1 and list(j.heads(branch)) == [TARGET]
    assert j.heads() == master and j.registry() == registry
    assert j.rows(
        "SELECT starting_branch,resolved_branch FROM version_inputs "
        "WHERE output_version_id=? ORDER BY alias",
        feature["version"],
    ) == [(branch, "master"), (branch, "master")]
    assert [r["customer_name"] for r in j.data(feature["version"])] == ["ADA", "GRACE"]
    j.report["master_heads"] = master
    j.report["isolated_heads"] = j.heads(branch)
    j.passed("isolated branch executes captured helper edit and uses labelled master fallback")
    upper = helper.read_text()
    helper.write_text(upper.replace(".str.to_uppercase()", ".str.to_lowercase()"))
    changed = j.build(TARGET, "--mode", "selected", "--branch", branch)
    assert j.job(changed)["state"] == "SUCCEEDED"
    assert j.job(changed)["version"] != feature["version"]
    assert [r["customer_name"] for r in j.data(j.job(changed)["version"])] == ["ada", "grace"]
    display_source = json.loads(browse.read_text())["source_snapshot_id"]
    assert (
        root / ".transflow/runtime/source-snapshots" / display_source / "files/src/common/names.py"
    ).read_text() == helper.read_text()
    helper.write_text(upper)
    restored = j.build(TARGET, "--mode", "selected", "--branch", branch)
    assert j.job(restored)["state"] == "CACHED"
    assert j.job(restored)["version"] == feature["version"]
    assert [r["customer_name"] for r in j.data(j.job(restored)["version"])] == ["ADA", "GRACE"]
    assert j.heads() == master
    j.passed("helper-only edit invalidates selected cache; reverting reuses its retained version")

    # Instrument only the invalid-input experiment, leaving the exact baseline untouched.
    consumer = root / "src/transforms/curated/customer_orders.py"
    original_consumer = consumer.read_text()
    marker = root / "consumer-called"
    consumer.write_text(
        original_consumer.replace(
            "    return (",
            f"    from pathlib import Path\n    Path({str(marker)!r}).touch()\n    return (",
        )
    )
    customers = root / "src/transforms/raw/customers.py"
    original_customers = customers.read_text()
    for label, replacement in (
        ("age 200", original_customers.replace("[0, 199]", "[0, 200]")),
        ("duplicate ID", original_customers.replace('"id": [1, 2]', '"id": [1, 1]')),
    ):
        customers.write_text(replacement)
        source = j.build("raw/customers", "--branch", branch)
        assert source["state"] == "SUCCEEDED"
        heads = j.heads(branch)
        failed = j.build(TARGET, "--mode", "selected", "--branch", branch, code=1)
        attempt = j.job(failed)["attempts"][0]
        assert failed["state"] == "FAILED" and not marker.exists()
        assert attempt["report"]["transform"] is None
        assert any(c["phase"] == "input" and c["outcome"] == "VIOLATION" for c in attempt["checks"])
        assert j.heads(branch) == heads and j.heads() == master
        assert j.invoke("build", "show", source["id"])["result"]["data"]["state"] == "SUCCEEDED"
        j.passed(f"{label} blocks invocation; last-good output and producer success survive")
    customers.write_text(original_customers)
    j.build("raw/customers", "--branch", branch)
    consumer.write_text(
        original_consumer.replace('E.primary_key("order_id")', 'E.col("amount").lt(0)')
    )
    heads = j.heads(branch)
    failed = j.build(TARGET, "--mode", "selected", "--branch", branch, "--force", code=1)
    attempt = j.job(failed)["attempts"][0]
    assert any(c["phase"] == "output" and c["outcome"] == "VIOLATION" for c in attempt["checks"])
    assert j.rows(
        "SELECT count(*) FROM failed_check_candidates WHERE attempt_id=?", attempt["id"]
    ) == [(1,)]
    assert j.heads(branch) == heads and j.heads() == master
    assert [r["customer_name"] for r in j.data(heads[TARGET])] == ["ADA", "GRACE"]
    consumer.write_text(original_consumer)
    j.passed("forced output FAIL quarantines the candidate and preserves last-good bytes/head")

    for label, reason, definitions in (
        (
            "unknown input",
            "input is unresolved",
            [
                "@transform(x=Input('missing/input'), output=Output('new/proposed'))"
                "\ndef new(x): return x\n",
            ],
        ),
        (
            "duplicate producer",
            "More than one producer",
            [
                "@transform(output=Output('curated/customer_orders'))\ndef duplicate(): pass\n",
                "@transform(output=Output('new/proposed'))\ndef new(): pass\n",
            ],
        ),
        (
            "cycle",
            "dependency cycle",
            [
                "@transform(x=Input('new/two'), output=Output('new/proposed'))"
                "\ndef new(x): return x\n",
                "@transform(x=Input('new/proposed'), output=Output('new/two'))"
                "\ndef two(x): return x\n",
            ],
        ),
    ):
        # One producer per module: reach the graph error, not the module-shape guard.
        extras = [root / f"src/invalid_{i}.py" for i in range(len(definitions))]
        for extra, definition in zip(extras, definitions, strict=True):
            extra.write_text("from transflow import transform, Input, Output\n" + definition)
        registry_bytes = (root / ".transflow/catalog.toml").read_bytes()
        browse_before = browse.read_bytes()
        attempts = j.rows("SELECT count(*) FROM attempts")
        for command in ("validate", "plan", "build"):
            args = [] if command == "validate" else [TARGET, "--branch", branch]
            failure = j.invoke(command, *args, "--python", sys.executable, code=1)
            assert any(reason in d["reason"] for d in failure["diagnostics"]), failure
            assert any(d["sources"] for d in failure["diagnostics"]), failure
        assert (root / ".transflow/catalog.toml").read_bytes() == registry_bytes
        assert j.rows("SELECT count(*) FROM attempts") == attempts
        assert browse.read_bytes() == browse_before
        assert j.heads(branch) == heads and j.heads() == master
        for extra in extras:
            extra.unlink()
        j.passed(f"{label} refuses validate/plan/build before registration or producer execution")

    # A stale draft must not replace the last structurally valid display graph.
    saved = j.invoke("plan", TARGET, "--branch", branch, "--python", sys.executable)["result"]
    browse_before = browse.read_bytes()
    helper.write_text(upper + "\n# edited after planning\n")
    j.invoke("build", "--plan", saved["plan_id"], code=1)
    assert browse.read_bytes() == browse_before
    helper.write_text(upper)
    j.passed("stale plan refusal preserves the browse graph")

    # Retained execution must not overwrite the current authoring display context.
    consumer.write_text("deliberately invalid current source !!!\n")
    replayed = j.invoke("build", "replay", first["id"], "--branch", "historical")["result"]["data"]
    assert replayed["state"] == "SUCCEEDED" and replayed["source"] == first["source"]
    assert browse.read_bytes() == browse_before
    assert consumer.read_text() == "deliberately invalid current source !!!\n"
    consumer.write_text(original_consumer)
    j.passed("replay executes frozen source without replacing current browse metadata")

    if git:
        assert committed is not None
        status = git_run("status", "--porcelain")
        historical = j.build(TARGET, "--git-ref", committed, "--branch", "committed")
        assert historical["state"] == "SUCCEEDED"
        assert j.data(j.job(historical)["version"]) == observed
        assert browse.read_bytes() == browse_before
        assert git_run("status", "--porcelain") == status
        assert git_run("branch", "--show-current").strip() == branch
        j.passed(
            "explicit Git-ref build uses committed helpers and preserves checkout/browse graph"
        )

        pending = root / "src/pending.py"
        pending.write_text(
            "import polars as pl\nfrom transflow import transform, Output\n"
            "@transform(output=Output('pending/unregistered'))\n"
            "def pending(): return pl.DataFrame({'id': [1]}).lazy()\n"
        )
        git_run("add", "src/pending.py")
        git_run("commit", "--no-gpg-sign", "-m", "Unregistered fixed-source fixture")
        registry_bytes = (root / ".transflow/catalog.toml").read_bytes()
        attempts = j.rows("SELECT count(*) FROM attempts")
        refused = j.invoke(
            "build",
            "pending/unregistered",
            "--git-ref",
            "HEAD",
            "--branch",
            "unregistered",
            "--python",
            sys.executable,
            code=1,
        )
        assert "registered IDs" in refused["diagnostics"][0]["reason"]
        assert (root / ".transflow/catalog.toml").read_bytes() == registry_bytes
        assert j.rows("SELECT count(*) FROM attempts") == attempts
        assert browse.read_bytes() == browse_before
        j.passed(
            "unregistered fixed source fails before registry edits, browse refresh or execution"
        )
    assert j.rows("SELECT count(*) FROM write_reservations") == [(0,)]
    assert j.registry() == registry
    if not git:
        assert not (root / ".git").exists() and shutil.which("git", path=j.env["PATH"]) is None
    exercise_imports(j)


def exercise_imports(j: Journey) -> None:
    """Explicit file imports become immutable input boundaries through the public CLI."""
    export = j.root.parent / "export"
    export.mkdir()

    def write_inputs(empty: bool = False) -> None:
        subprocess.run(
            [
                sys.executable,
                "-I",
                "-c",
                "import polars as p,sys; from pathlib import Path; "
                "d=p.DataFrame({'id':p.Series([],dtype=p.Int64) "
                "if sys.argv[2]=='empty' else [1,2]}); "
                "[d.write_parquet(Path(sys.argv[1])/name) "
                "for name in ('one.parquet','two.parquet')]",
                str(export),
                "empty" if empty else "rows",
            ],
            check=True,
            timeout=30,
        )

    write_inputs()
    (j.root / "src/import_consumer.py").write_text(
        "from transflow import transform, Input, Output, Check\n"
        "from transflow import expectations as E\nfrom transflow.catalog import C\n"
        "@transform(rows=Input(C.raw.imported, "
        "checks=Check(E.row_count().gt(0), name='Nonempty')), "
        "output=Output('curated/imported'))\ndef consume(rows): return rows\n"
    )

    def imported(*extra: str, code: int = 0) -> dict[str, Any]:
        return j.invoke(
            "dataset",
            "import",
            "raw/imported",
            "--path",
            str(export / "*.parquet"),
            "--branch",
            "imports",
            "--python",
            sys.executable,
            *extra,
            code=code,
        )

    first = imported()["result"]
    assert first["published"] and first["row_count"] == "4" and first["file_count"] == "2"
    assert j.heads("imports")["raw/imported"] == first["version_id"]
    provenance = json.loads(
        j.rows(
            "SELECT manifest_json FROM source_snapshots WHERE id=?", first["source_snapshot_id"]
        )[0][0]
    )
    assert provenance["source_root"] == str(export)
    assert provenance["source_files"] == ["one.parquet", "two.parquet"]
    assert j.rows(
        "SELECT import_id,attempt_id FROM dataset_versions WHERE id=?", first["version_id"]
    ) == [(first["import_id"], None)]
    assert j.rows(
        "SELECT count(*) FROM version_check_results WHERE version_id=?", first["version_id"]
    ) == [(0,)]
    assert not list((j.root / ".transflow/runtime/import-staging").iterdir())
    j.passed("multi-file import publishes an immutable version and explicit import provenance")
    for path in export.iterdir():
        path.unlink()
    built = j.build("curated/imported", "--branch", "imports", "--force")
    assert built["state"] == "SUCCEEDED" and len(built["jobs"]) == 1
    assert j.job(built, "curated/imported")["inputs"][0]["version"] == first["version_id"]
    j.passed(
        "full force consumes imported bytes after original files disappear without refreshing them"
    )
    prior = j.heads("imports")
    (export / "bad.parquet").write_bytes(b"invalid parquet")
    imported(code=1)
    assert j.heads("imports") == prior
    (export / "bad.parquet").unlink()
    j.passed("corrupt reimport preserves every previous head")
    write_inputs(empty=True)
    prepared = imported("--prepare-only")["result"]
    assert not prepared["published"] and prepared["row_count"] == "0"
    assert j.heads("imports") == prior
    second = imported()["result"]
    assert second["published"] and second["row_count"] == "0"
    assert (
        second["dataset_id"] == first["dataset_id"] and second["version_id"] != first["version_id"]
    )
    j.passed(
        "preparation remains nonpublishing and a valid empty reimport advances the same identity"
    )
    failure = j.build("curated/imported", "--branch", "imports", code=1)
    assert failure["state"] == "FAILED"
    assert j.heads("imports")["curated/imported"] == prior["curated/imported"]
    assert j.job(failure, "curated/imported")["attempts"][0]["report"]["transform"] is None
    j.passed("nonempty consumer expectation blocks an empty imported version before the function")
    pinned = j.build(
        "curated/imported",
        "--branch",
        "imports",
        "--force",
        "--pin",
        f"curated/imported#rows={first['version_id']}",
    )
    assert pinned["state"] == "SUCCEEDED"
    replay = j.invoke("build", "replay", built["id"], "--branch", "import-replay")["result"]["data"]
    assert (
        replay["state"] == "SUCCEEDED"
        and replay["jobs"][0]["inputs"][0]["version"] == first["version_id"]
    )
    j.passed("imported historical pins and replay retain original immutable bytes")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cli", type=Path)
    parser.add_argument("wheel", type=Path)
    parser.add_argument("wheelhouse", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--keep-failed", action="store_true", help="Retain failed synthetic workspace"
    )
    args = parser.parse_args()
    cli, wheel, wheelhouse = [
        p.resolve(strict=True) for p in (args.cli, args.wheel, args.wheelhouse)
    ]
    git = shutil.which("git")
    if git is None:
        parser.error("Git is required for the separate Git-feature variant")

    def digest(path: Path) -> str:
        with path.open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()

    report: dict[str, Any] = {
        "tasks": ["T068", "T073"],
        "gate_complete": False,
        "variants": [],
        "platform": platform.platform(),
        "python": platform.python_version(),
        "cli_sha256": digest(cli),
        "wheel_sha256": digest(wheel),
        "fixture_sha256": {
            str(p.relative_to(FIXTURE)): digest(p)
            for p in sorted(FIXTURE.rglob("*"))
            if p.is_file()
        },
        "limits": [
            "Native editor qualification and automatic editor overlays are deferred to G3",
            "Foreign-data scenarios run in the separate direct-read suite",
            "Dataset preview/history and browser serving are not implemented",
            "This run does not establish remote CI or release qualification",
        ],
    }
    try:
        for name, executable in (("no-git", None), ("git-feature", git)):
            variant: dict[str, Any] = {"name": name, "commands": [], "cases": []}
            report["variants"].append(variant)
            scratch = tempfile.TemporaryDirectory(
                prefix=f"tf-core-{name}-", delete=not args.keep_failed
            )
            with scratch as temporary:
                exercise(
                    Journey(
                        Path(temporary).resolve() / "workspace",
                        cli,
                        variant,
                        executable is not None,
                    ),
                    wheel,
                    wheelhouse,
                    executable,
                )
                scratch.cleanup()
            variant["passed"] = len(variant["cases"])
        report["passed"] = sum(v["passed"] for v in report["variants"])
    except Exception as error:
        report["failure"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "report": str(args.output)}))


if __name__ == "__main__":
    main()
