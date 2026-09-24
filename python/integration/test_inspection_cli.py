"""Public nonexecuting inspection commands through the installed worker."""

import json
import sqlite3
import sys
from contextlib import closing
from pathlib import Path
from typing import Any
from uuid import uuid4

from test_catalog_cli import CLI, DECLARATION, cli, source
from test_catalog_cli import workspace as workspace
from test_packaging import run
from test_packaging import wheel as wheel


def diamond(root: Path) -> None:
    source(root, "root.py", 'print("import noise must not reach JSON")\n' + DECLARATION)
    for name, inputs in [
        ("left", ["raw/orders"]),
        ("right", ["raw/orders"]),
        ("end", ["curated/left", "curated/right"]),
    ]:
        bindings = ", ".join(f'x{n}=Input("{p}")' for n, p in enumerate(inputs))
        source(
            root,
            f"{name}.py",
            "from transflow import transform, Input, Output\n"
            + f'@transform({bindings}, output=Output("curated/{name}"))\n'
            + f"def produce({', '.join(f'x{n}' for n in range(len(inputs)))}):\n"
            + '    raise AssertionError("must not execute")\n',
        )


def inspect(root: Path, name: str, *args: str, ok: bool = True) -> dict[str, Any]:
    value: dict[str, Any] = cli(root, name, *args, "--python", sys.executable, ok=ok)["result"]
    return value


def test_public_plan_defaults_deduplicates_and_keeps_symbolic_parents(workspace: Path) -> None:
    diamond(workspace)
    before = (workspace / ".transflow/catalog.toml").read_bytes()
    plan = inspect(workspace, "plan", "curated/end", "--target", "curated/end")
    assert plan["mode"] == "Full" and plan["branch"] == "master"
    assert plan["targets"] == ["curated/end"]
    assert len(plan["writes"]) == 4 and len(plan["pending_registrations"]) == 4
    assert plan["reads"] == [] and plan["branch_would_be_created"]
    assert len(plan["digest"]) == 64 and int(plan["expires_us"]) > int(plan["created_us"])
    assert all(b["kind"] == "planned" for w in plan["writes"] for b in w["bindings"])
    assert (workspace / ".transflow/catalog.toml").read_bytes() == before
    with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
        for table in ["attempts", "dataset_versions", "datasets", "data_branches", "builds"]:
            assert db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] == 0
    why = inspect(workspace, "why", "curated/end", "--force")
    assert why["kind"] == "why" and why["plan"]["force"]
    assert why["status"]["materialization"] == "NeverBuilt"
    blocked = inspect(workspace, "why", "curated/end", "--mode", "selected")
    assert blocked["plan"] is None and blocked["planning_error"]
    assert blocked["status"]["materialization"] == "NeverBuilt"
    human = run(
        [
            str(CLI),
            "--workspace",
            str(workspace),
            "plan",
            "curated/end",
            "--python",
            sys.executable,
        ],
        workspace,
    )
    assert "Pending registration: curated/end" in human.stdout
    assert "symbolic in-build" in human.stdout and "No producers ran" in human.stdout
    assert "import noise" not in human.stdout


def test_public_depth_and_complete_pagination(workspace: Path) -> None:
    diamond(workspace)
    for direction, start in [("upstream", "curated/end"), ("downstream", "raw/orders")]:
        for depth, count in [("0", 1), ("1", 3), ("2", 4), (None, 4)]:
            result = inspect(workspace, direction, start, *(["--depth", depth] if depth else []))
            assert len(result["nodes"]) == count
            assert result["nodes"][0]["depth"] == "0"
            assert result["scope_complete"] is (count == 4)
            assert result["delivery_complete"] and not result["external_expanded"]
            assert result["omitted_nodes"] == str(4 - count)
    # More than one service page, with sufficient aliases to exceed the old 1 MiB CLI cap.
    bindings = ", ".join(
        f"alias_{n}_" + "x" * 100 + '=Input("raw/orders", role="validation")' for n in range(2200)
    )
    source(
        workspace,
        "many.py",
        "from transflow import transform, Input, Output\n"
        + f'@transform({bindings}, output=Output("curated/many"))\n'
        f'def many(): raise AssertionError("must not execute")\n',
    )
    result = inspect(workspace, "upstream", "curated/many")
    assert len(result["nodes"]) == 2 and len(result["edges"]) == 2200
    assert len({e["alias"] for e in result["edges"]}) == 2200
    (workspace / "src/many.py").unlink()
    for n in range(1100):
        source(
            workspace,
            f"wide_{n}.py",
            "from transflow import transform, Input, Output\n"
            + f'@transform(rows=Input("raw/orders"), output=Output("wide/n{n}"))\n'
            + 'def produce(rows): raise AssertionError("must not execute")\n',
        )
    wide = inspect(workspace, "downstream", "raw/orders")
    assert wide["total_nodes"] == "1104" and len(wide["nodes"]) == 1104
    assert wide["total_edges"] == "1104" and wide["delivery_complete"]


def test_public_parameters_resources_and_selection(workspace: Path) -> None:
    source(
        workspace,
        "root.py",
        "from transflow import transform, Output, Parameter\n"
        '@transform(output=Output("raw/orders"), params={"limit":Parameter('
        '{"type":"i64"}, {"type":"i64","value":"1"})}, '
        'resources={"wall_timeout_seconds":90})\n'
        'def produce(*, ctx): raise AssertionError("must not execute")\n',
    )
    p = inspect(
        workspace,
        "plan",
        "raw/orders",
        "--mode",
        "selected",
        "--param",
        "limit=7",
        "--timeout-seconds",
        "0",
        "--validation-timeout-seconds",
        "12",
        "--no-fallback",
    )
    assert p["writes"][0]["parameters"]["limit"] == {"type": "i64", "value": "7"}
    assert p["writes"][0]["resources"]["timeout_seconds"] == "0"
    assert p["writes"][0]["resources"]["validation_timeout_seconds"] == "12"
    assert p["writes"][0]["resources"]["timeout_origin"] == "Explicit"
    defaults = inspect(workspace, "plan", "raw/orders")["writes"][0]["resources"]
    assert defaults["timeout_seconds"] == "90" and defaults["timeout_origin"] == "Definition"
    assert defaults["validation_timeout_seconds"] == "3600"
    assert defaults["interactive_timeout_seconds"] == "30"
    assert defaults["discovery_timeout_seconds"] == "3600"
    assert defaults["memory_budget_mib"] is None and defaults["memory_origin"] is None
    assert defaults["memory_enforcement"] == "none"
    assert 1 <= int(defaults["worker_threads"]) <= int(defaults["cpu_tokens"])
    with (workspace / "workspace.toml").open("a") as config:
        config.write(
            "\n[execution]\nwall_timeout_seconds=0\nmax_jobs=2\ncpu_tokens=6\n"
            "memory_budget_mib=128\n"
        )
    configured = inspect(workspace, "plan", "raw/orders")["writes"][0]["resources"]
    assert configured["discovery_timeout_seconds"] == "0"
    assert configured["discovery_timeout_origin"] == "Workspace"
    assert (
        configured["timeout_seconds"] == "90"
    )  # Definition overrides execution workspace default.
    assert configured["memory_budget_mib"] == "128"
    assert configured["memory_enforcement"] == "estimated_reservation"
    assert configured["cpu_origin"] == "Workspace" and configured["worker_threads"] == "3"
    assert p["fallback_override"] == []
    assert (
        inspect(
            workspace,
            "plan",
            "raw/orders",
            "--param",
            'raw/orders#limit={"type":"i64","value":"9223372036854775807"}',
        )["writes"][0]["parameters"]["limit"]["value"]
        == "9223372036854775807"
    )
    for args in [
        ("--param", "missing=2"),
        ("--param", 'limit="wrong"'),
        ("--exclude", "raw/orders"),
        ("--boundary", "raw/orders"),
    ]:
        assert (
            cli(workspace, "plan", "raw/orders", *args, "--python", sys.executable, ok=False)[
                "exit_status"
            ]
            == 1
        )


def test_git_source_and_branch_selection_leave_checkout_untouched(workspace: Path) -> None:
    source(workspace, "root.py", DECLARATION)

    def git(*args: str) -> str:
        return run(["git", *args], workspace).stdout.strip()

    git("init", "-b", "feature/data")
    git(
        "add",
        "workspace.toml",
        ".transflow/catalog.toml",
        "requirements.in",
        "requirements.lock",
        "src",
    )
    git(
        "-c",
        "user.name=Synthetic",
        "-c",
        "user.email=synthetic@example.invalid",
        "commit",
        "-m",
        "fixture",
    )
    commit = git("rev-parse", "HEAD")
    assert inspect(workspace, "plan", "raw/orders")["branch"] == "feature/data"
    source(workspace, "root.py", "invalid current Python !!!")
    before = git("status", "--porcelain")
    p = inspect(workspace, "plan", "raw/orders", "--git-ref", commit, "--branch", "review")
    assert p["branch"] == "review" and p["source_commit"] == commit
    assert p["source_selector"] == {"kind": "git_ref", "ref": commit}
    assert (
        inspect(workspace, "upstream", "raw/orders", "--git-ref", commit, "--branch", "review")[
            "total_nodes"
        ]
        == "1"
    )
    assert git("status", "--porcelain") == before
    assert git("rev-parse", "HEAD") == commit
    git("checkout", "--detach", "HEAD")
    assert (
        cli(workspace, "upstream", "raw/orders", "--python", sys.executable, ok=False)[
            "exit_status"
        ]
        == 1
    )


def test_foreign_boundaries_and_unrelated_invalid_graph(workspace: Path) -> None:
    provider, dataset, registration = uuid4(), uuid4(), uuid4()
    registry = workspace / ".transflow/catalog.toml"
    registry.write_text(
        registry.read_text() + f"\n"
        f"[[external_registrations]]\n"
        f'id="{registration}"\n'
        f'alias="external/demo/value"\n'
        f'provider_workspace_id="{provider}"\n'
        f'provider_dataset_id="{dataset}"\n'
        f'provider_display_path="raw/value"\n'
        f'default_branch="master"\n'
    )
    source(
        workspace,
        "consumer.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("external/demo/value"), output=Output("curated/end"))\n'
        'def produce(rows): raise AssertionError("must not execute")\n',
    )
    graph = inspect(workspace, "upstream", "curated/end")
    assert graph["nodes"][1]["external"]
    failed = cli(workspace, "plan", "curated/end", "--python", sys.executable, ok=False)
    assert "Provider locator is missing" in json.dumps(failed)
    why = inspect(workspace, "why", "external/demo/value")
    assert why["status"] is None and why["planning_error"]
    source(workspace, "local.py", DECLARATION)
    assert inspect(workspace, "plan", "raw/orders")["writes"][0]["path"] == "raw/orders"
    source(workspace, "bad.py", "invalid Python !!!")
    assert (
        cli(
            workspace,
            "upstream",
            "curated/end",
            "--depth",
            "0",
            "--python",
            sys.executable,
            ok=False,
        )["exit_status"]
        == 1
    )

    source(
        workspace,
        "bad.py",
        "from transflow import transform, Input, Output\n"
        '@transform(rows=Input("bad/cycle"), output=Output("bad/cycle"))\n'
        'def cycle(rows): raise AssertionError("must not execute")\n',
    )
    for command in ["plan", "why", "upstream"]:
        failure = cli(workspace, command, "raw/orders", "--python", sys.executable, ok=False)
        assert failure["diagnostics"][0]["code"] == "TF_GRAPH_CYCLE"
        assert failure["diagnostics"][0]["sources"][0]["path"] == "src/bad.py"


def test_selected_between_fallback_and_exact_pins_use_real_retained_versions(
    workspace: Path,
) -> None:
    config = workspace / "workspace.toml"
    import re

    config.write_text(
        re.sub(
            r'workspace_id = "[^"]+"',
            'workspace_id = "01010101-0101-0101-0101-010101010101"',
            config.read_text(),
        )
    )
    registry = workspace / ".transflow/catalog.toml"
    registry.write_text(
        'format_version=1\n[[datasets]]\nid="0a0a0a0a-0a0a-0a0a-0a0a-0a0a0a0a0a0a"\n'
        'path="raw/orders"\nkind="transform"\n'
    )
    diamond(workspace)
    seeded = json.loads(
        run([str(CLI.parent / "examples/inspection_fixture"), str(workspace)], workspace).stdout
    )
    for mode, target, extra, writes, reads in [
        ("selected", "curated/left", [], 1, 1),
        ("full", "curated/left", ["--exclude", "raw/orders", "--force"], 1, 1),
        ("between", "curated/end", ["--boundary", "raw/orders", "--force"], 3, 2),
    ]:
        result = inspect(workspace, "plan", target, "--mode", mode, *extra)
        assert len(result["writes"]) == writes and len(result["reads"]) == reads
        assert all(r["version"] == seeded["head"] for r in result["reads"])
        assert result["warnings"]
    fallback = inspect(
        workspace,
        "plan",
        "curated/left",
        "--mode",
        "selected",
        "--branch",
        "feature",
        "--fallback",
        "master",
    )
    assert fallback["reads"][0]["starting_branch"] == "feature"
    assert fallback["reads"][0]["resolved_branch"] == "master"
    pinned = inspect(
        workspace,
        "plan",
        "curated/left",
        "--mode",
        "selected",
        "--no-fallback",
        "--branch",
        "feature",
        "--pin",
        f"curated/left#x0={seeded['older']}",
    )
    assert pinned["reads"][0]["version"] == seeded["older"]
    assert pinned["reads"][0]["resolution"] == "exact_pin"
    why = inspect(workspace, "why", "raw/orders", "--mode", "selected")
    parent = why["status"]
    assert parent["materialization"] == "Available" and parent["version"] == seeded["head"]
    assert parent["direct_logic"] == "Unknown"
    for options in [
        ["--branch", "feature", "--no-fallback"],
        ["--boundary-policy", "require_current"],
        ["--pin", f"raw/orders={uuid4()}"],
    ]:
        assert (
            cli(
                workspace,
                "plan",
                "curated/left",
                "--mode",
                "selected",
                *options,
                "--python",
                sys.executable,
                ok=False,
            )["exit_status"]
            == 1
        )
    assert (
        cli(
            workspace,
            "plan",
            "curated/left",
            "--target",
            "raw/orders",
            "--pin",
            f"raw/orders={seeded['older']}",
            "--python",
            sys.executable,
            ok=False,
        )["exit_status"]
        == 1
    )
