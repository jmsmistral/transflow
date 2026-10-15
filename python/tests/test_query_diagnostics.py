"""Private query errors retain actionable SQL evidence without dumping unexpected failures."""

import io
from typing import Any

import pytest
from transflow_worker import query
from transflow_worker.scratchpad import QueryError, diagnostic
from transflow_worker.sql_policy import SqlPolicyError
from transflow_worker.wire import read_frame


@pytest.mark.parametrize(
    ("error", "expected"),
    [
        (QueryError('Binder Error: Referenced column "ida" not found'), 'Referenced column "ida"'),
        (SqlPolicyError("SQL function is outside the reviewed allowlist"), "reviewed allowlist"),
        (RuntimeError("private traceback /secrets/file"), "Query failed; check SQL"),
    ],
)
def test_validated_query_error_terminal(
    monkeypatch: pytest.MonkeyPatch, error: Exception, expected: str
) -> None:
    def fail(*args: Any, **kwargs: Any) -> None:
        assert kwargs == {"detailed_errors": True}
        raise error

    monkeypatch.setattr(query, "execute", fail)
    stream = io.BytesIO()
    identity = "11111111-1111-4111-8111-111111111111"
    assert (
        query._session(
            {"request_id": identity, "attempt_id": identity, "query": {}, "limits": {}}, stream
        )
        == 1
    )
    stream.seek(0)
    frames = []
    while (frame := read_frame(stream)) is not None:
        frames.append(frame.as_json()["message"])
    assert frames[-1]["code"] == "query_failed"
    assert expected in frames[-1]["message"]
    assert "private traceback" not in frames[-1]["message"]
    assert frames[-1]["retryable"] is False


def test_diagnostic_bound_preserves_utf8_and_useful_line_breaks() -> None:
    result = diagnostic("Binder Error\n\tida\x00\x1b" + "é" * 4096)
    assert len(result.encode()) <= 4096
    assert result.startswith("Binder Error\n\tidaé") and result.endswith("…")
    assert "\x00" not in result and "\x1b" not in result
