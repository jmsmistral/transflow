"""Actual loopback coordinator security, context fencing and durable retry journeys."""

from __future__ import annotations

import sqlite3
import sys
from contextlib import closing
from pathlib import Path
from uuid import uuid4

from api_client import Client, coordinator
from test_catalog_cli import CLI, DECLARATION, cli, source
from test_catalog_cli import workspace as workspace
from test_packaging import wheel as wheel


def test_http_auth_context_and_persistent_retries(workspace: Path) -> None:
    with coordinator(workspace, CLI) as registration:
        client = Client(registration)
        client.call("GET", "/api/v1/capabilities", auth=False, status=401)
        client.call(
            "GET",
            "/api/v1/capabilities",
            headers={"Origin": "https://attacker.invalid"},
            status=403,
        )
        client.call("GET", "/api/v1/capabilities", headers={"Host": "attacker.invalid"}, status=403)
        capabilities = client.call("GET", "/api/v1/capabilities")["data"]
        assert capabilities["limits"]["page_max"] == 200
        context = client.context()
        key = str(uuid4())
        created = client.mutate(
            "/api/v1/branches", {"operation": "create", "name": "review"}, context=context, key=key
        )
        assert created["data"]["applied"]
        assert (
            client.mutate(
                "/api/v1/branches",
                {"operation": "create", "name": "review"},
                context=context,
                key=key,
            )["data"]
            == created["data"]
        )
        client.mutate(
            "/api/v1/branches",
            {"operation": "create", "name": "different"},
            context=context,
            key=key,
            status=409,
        )
        client.mutate(
            "/api/v1/branches",
            {"operation": "create", "name": "stale"},
            context=context,
            status=409,
        )
        client.call(
            "GET",
            "/api/v1/datasets",
            query={"branch": "main", "context": context["context"]["fingerprint"]},
            status=409,
        )
        launch = client.call("POST", "/api/v1/sessions/launch", {})["data"]
        exchanged = client.call(
            "POST",
            "/api/v1/sessions/exchange",
            {"code": launch["code"]},
            auth=False,
            headers={"Origin": f"http://{client.endpoint}"},
        )
        assert exchanged["data"]["csrf"]
        client.call(
            "POST",
            "/api/v1/sessions/exchange",
            {"code": launch["code"]},
            auth=False,
            headers={"Origin": f"http://{client.endpoint}"},
            status=401,
        )
    with coordinator(workspace, CLI) as registration:
        client = Client(registration)
        assert (
            client.mutate(
                "/api/v1/branches",
                {"operation": "create", "name": "review"},
                context=context,
                key=key,
            )["data"]
            == created["data"]
        )
        with closing(sqlite3.connect(workspace / ".transflow/runtime/catalog.sqlite")) as db:
            assert db.execute(
                "SELECT count(*) FROM data_branches WHERE name='review'"
            ).fetchone() == (1,)


def test_http_shared_preparation_and_retained_contexts(workspace: Path) -> None:
    source(workspace, "orders.py", DECLARATION)
    with coordinator(workspace, CLI) as registration:
        client = Client(registration)
        body = {"python": sys.executable}
        pending = client.call("POST", "/api/v1/catalog/diff", body, query={"branch": "main"})
        assert pending["data"]["registrations"]["total"] == "1"
        synced = client.mutate("/api/v1/catalog/sync", body)
        assert synced["data"]["registrations"]["total"] == "1"
        page = client.call("GET", "/api/v1/datasets", query={"branch": "main", "limit": "1"})
        assert page["data"]["entries"][0]["path"] == "raw/orders"
        graph = client.call(
            "GET", "/api/v1/lineage", query={"branch": "main", "start": "raw/orders"}
        )
        assert graph["data"]["total_nodes"] == 1
        snapshot = graph["context"]["source"]
        excerpt = client.call(
            "GET", f"/api/v1/source/{snapshot}", query={"branch": "main", "path": "src/orders.py"}
        )
        assert excerpt["data"]["text"] == DECLARATION.rstrip()
        client.call(
            "GET",
            f"/api/v1/source/{uuid4()}",
            query={"branch": "main", "path": "src/orders.py"},
            status=409,
        )
        client.call(
            "GET",
            f"/api/v1/source/{snapshot}",
            query={"branch": "main", "path": "../../outside"},
            status=404,
        )
        selection = {"branch": "main", "targets": ["raw/orders"], "python": sys.executable}
        draft = client.mutate("/api/v1/plans", selection)["data"]
        exact = client.call(
            "GET",
            f"/api/v1/plans/{draft['plan_id']}",
            query={"branch": "main", "plan": draft["plan_id"]},
        )
        assert exact["data"] == draft
        client.call(
            "GET",
            f"/api/v1/plans/{draft['plan_id']}",
            query={"branch": "other", "plan": draft["plan_id"]},
            status=409,
        )
        # Read-only retained source/graph routes cannot import edited, failing local code.
        source(workspace, "orders.py", "raise RuntimeError('must not import during GET')\n")
        historical = client.call(
            "GET",
            "/api/v1/lineage",
            query={"branch": "main", "plan": draft["plan_id"], "start": "raw/orders"},
        )
        assert historical["data"]["total_nodes"] == 1
        client.mutate(
            "/api/v1/builds",
            {"kind": "plan", "plan_id": draft["plan_id"]},
            plan=draft["plan_id"],
            status=409,
        )
        client.mutate(
            "/api/v1/builds",
            {"kind": "plan", "plan_id": draft["plan_id"], "force": True},
            plan=draft["plan_id"],
            status=400,
        )
    # CLI sees the exact same registered identities; source is restored only for this test.
    source(workspace, "orders.py", DECLARATION)
    assert cli(workspace, "catalog", "list")["result"]["entries"][0]["path"] == "raw/orders"


def test_http_keyset_pages_bind_filters_and_revisions(workspace: Path) -> None:
    for n in range(3):
        source(workspace, f"items_{n}.py", DECLARATION.replace("raw/orders", f"raw/items_{n}"))
    with coordinator(workspace, CLI) as registration:
        client = Client(registration)
        client.mutate("/api/v1/catalog/sync", {"python": sys.executable})
        first = client.call("GET", "/api/v1/datasets", query={"branch": "main", "limit": "1"})
        cursor = first["data"]["next_cursor"]
        second = client.call(
            "GET",
            "/api/v1/datasets",
            query={"branch": "main", "limit": "1", "cursor": cursor},
        )
        assert first["data"]["entries"][0]["path"] == "raw/items_0"
        assert second["data"]["entries"][0]["path"] == "raw/items_1"
        assert second["context"] == first["context"]
        client.call(
            "GET",
            "/api/v1/datasets",
            query={"branch": "main", "cursor": cursor, "filter": "items"},
            status=409,
        )
        filtered = client.call(
            "GET", "/api/v1/datasets", query={"branch": "main", "source_path": "items_2.py"}
        )
        assert filtered["data"]["total"] == "1"
        client.call("GET", "/api/v1/datasets", query={"branch": "main", "limit": "201"}, status=400)
        client.call("GET", "/api/v1/datasets", query={"branch": "main", "unknown": "x"}, status=400)
        client.mutate("/api/v1/branches", {"operation": "create", "name": "new_revision"})
        client.call(
            "GET", "/api/v1/datasets", query={"branch": "main", "cursor": cursor}, status=409
        )
