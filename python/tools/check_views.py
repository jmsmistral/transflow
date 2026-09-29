"""Saved-view journeys against the real authenticated coordinator and SQLite store."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any
from uuid import uuid4

from api_client import Client


def exercise(client: Client, root: Path) -> dict[str, Any]:
    """Persist a bounded synthetic view without changing executable state."""
    import sqlite3

    def counts() -> tuple[int, ...]:
        with sqlite3.connect(root / ".transflow/runtime/catalog.sqlite") as db:
            return tuple(
                db.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                for table in ("builds", "dataset_versions", "dataset_heads")
            )

    before = counts()
    context = client.context()
    graph = client.call(
        "GET",
        "/api/v1/lineage",
        query={"branch": "main", "start": "curated/external", "direction": "upstream"},
    )["data"]
    ids = [node["identity"] for node in graph["nodes"]]
    view: dict[str, Any] = {
        "format_version": 1,
        "id": str(uuid4()),
        "revision": 0,
        "name": "Synthetic saved lineage",
        "description": "Presentation only",
        "datasets": [
            {"identity": identity, "position": {"x": i * 300.5, "y": 90}}
            for i, identity in enumerate(ids)
        ],
        "viewport": {"x": -10, "y": 20, "zoom": 0.8},
        "colour": "resource",
        "selector": {
            "branch": "main",
            "fallback": context["context"]["fallback_policy"][1:],
        },
    }
    key = str(uuid4())
    saved: dict[str, Any] = client.mutate("/api/v1/views", view, context=context, key=key)["data"]
    assert saved["revision"] == 1
    assert client.mutate("/api/v1/views", view, context=context, key=key)["data"] == saved
    client.mutate("/api/v1/views", view, context=context, status=409)
    assert client.context()["context"]["fingerprint"] == context["context"]["fingerprint"]
    assert client.call("GET", f"/api/v1/views/{view['id']}")["data"] == saved
    assert saved["id"] in [v["id"] for v in client.call("GET", "/api/v1/views")["data"]["views"]]
    reopened = client.call(
        "GET", "/api/v1/lineage", query={"branch": "main", "lookup": json.dumps(ids)}
    )["data"]
    assert sorted(ids) == sorted(n["identity"] for n in reopened["nodes"])
    client.call(
        "GET", "/api/v1/context", query={"branch": "main", "source_graph": "a" * 64}, status=400
    )
    for obsolete in ("groups", "annotations", "filters"):
        client.mutate("/api/v1/views", {**saved, obsolete: []}, status=400)
    client.mutate(
        "/api/v1/views",
        {**saved, "selector": {**saved["selector"], "mode": "snapshot"}},
        status=400,
    )
    invalid = {**saved, "build": "not an operation"}
    client.mutate("/api/v1/views", invalid, status=400)
    assert counts() == before
    return saved
