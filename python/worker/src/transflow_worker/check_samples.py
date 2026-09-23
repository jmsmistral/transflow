"""Private bounded diagnostics. Counts are computed independently; only allowed columns arrive."""

from __future__ import annotations

import json
from typing import Any

from .wire import validate_document


def cell(text: str | None, logical: dict[str, Any]) -> dict[str, Any]:
    if text is None:
        return {"type": "null"}
    kind = logical["type"]
    result = {**logical, "value": text}
    if kind == "bool":
        if text not in {"true", "false"}:
            raise ValueError("Invalid boolean sample")
        result["value"] = text == "true"
    elif kind in {"f32", "f64"}:
        result["value"] = {"nan": "NaN", "inf": "Infinity", "-inf": "-Infinity"}.get(text, text)
    elif kind == "timestamp":
        # DuckDB VARCHAR retains ns; never round-trip through Python datetime/float.
        text = text.replace(" ", "T")
        if logical["timezone"] is not None:
            if not text.endswith("+00"):
                raise ValueError("Invalid timestamp sample")
            text = text[:-3]
        precision = {"s": 0, "ms": 3, "us": 6, "ns": 9}[logical["unit"]]
        date, _, fraction = text.partition(".")
        text = date + ("." + fraction.ljust(precision, "0") if precision else "")
        result["value"] = text + ("Z" if logical["timezone"] is not None else "")
    validate_document("ScalarValue", result)
    return result


def execute(db: Any, query: dict[str, Any]) -> dict[str, Any]:
    result: dict[str, Any] = {
        "columns": query["columns"],
        "rows": [],
        "limit": query["limit"],
        "truncated": False,
        "reason": query["reason"],
    }
    if query["reason"] is not None:
        return result
    cursor = db.execute(query["sql"], [v["value"] for v in query["parameters"]])
    for _ in range(query["limit"] + 1):
        raw = cursor.fetchone()
        if raw is None:
            break
        if len(raw) != len(query["columns"]) or any(
            v is not None and (not isinstance(v, str) or len(v.encode()) > 4096) for v in raw
        ):
            raise ValueError("Invalid bounded sample")
        if len(result["rows"]) == query["limit"]:
            result["truncated"] = True
            break
        row = [cell(v, f["logical_type"]) for v, f in zip(raw, query["columns"], strict=True)]
        result["rows"].append(row)
        if len(json.dumps(result, ensure_ascii=False).encode()) > 60000:
            result["rows"].pop()
            result["truncated"] = True
            break
    validate_document("CheckSampleV1", result)
    return result
