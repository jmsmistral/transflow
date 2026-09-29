"""Deterministic coverage of the native probe's pre-spawn recovery window."""

import json
from pathlib import Path

import pytest
from check_queries import wait_for_helper


def test_helper_waits_for_spawn_identity(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    records = iter(
        [{"pid": 0, "start": ""}, {"pid": 123, "start": ""}, {"pid": 123, "start": "started"}]
    )
    sleeps: list[float] = []

    def advance(seconds: float) -> None:
        sleeps.append(seconds)
        (tmp_path / "recovery.json").write_text(json.dumps(next(records)))

    monkeypatch.setattr("check_queries.time.sleep", advance)
    assert wait_for_helper(tmp_path) == 123
    assert len(sleeps) == 3


@pytest.mark.parametrize("placeholder", [False, True])
def test_helper_start_deadline_is_bounded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, placeholder: bool
) -> None:
    if placeholder:
        (tmp_path / "recovery.json").write_text('{"pid":0,"start":""}')
    ticks = iter([0.0, 0.0, 15.0])
    monkeypatch.setattr("check_queries.time.monotonic", lambda: next(ticks))
    monkeypatch.setattr("check_queries.time.sleep", lambda _: None)
    with pytest.raises(AssertionError, match="spawned process identity"):
        wait_for_helper(tmp_path)
