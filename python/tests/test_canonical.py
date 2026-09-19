"""Fixed canonical encodings/hashes and boundaries shared with Rust and TypeScript."""

import json
from copy import deepcopy
from io import BytesIO
from pathlib import Path
from typing import Any

import pytest
from transflow_worker.canonical import (
    MAX_CANONICAL_BYTES,
    CanonicalError,
    DigestKind,
    artifact_digest,
    canonical_json,
    catalog_fingerprint,
    content_digest,
    file_digest,
    semantic_fingerprint,
    verify_catalog_fingerprint,
)
from transflow_worker.wire import validate_document

# Any is confined to decoded fixture JSON; production canonical input is object.
FIXTURES: dict[str, Any] = json.loads(
    (Path(__file__).resolve().parents[2] / "schemas/fixtures/canonical-v1.json").read_text()
)


@pytest.mark.parametrize("case", FIXTURES["cases"], ids=lambda case: case["name"])
def test_fixed_encodings_and_five_domain_digests(case: dict[str, Any]) -> None:
    if case["name"] == "lossless-values":
        for value in case["value"]:
            validate_document("WireValue", value)
    assert canonical_json(case["value"]) == case["canonical"].encode()
    for kind in DigestKind:
        if kind is DigestKind.FILE:
            continue
        digest = content_digest(kind, case["value"])
        assert digest.kind is kind
        assert digest.hex == case["sha256"][kind.value.split(".")[1]]


@pytest.mark.parametrize("case", FIXTURES["invalid"], ids=lambda case: case["name"])
def test_invalid_shared_numbers(case: dict[str, Any]) -> None:
    with pytest.raises(CanonicalError):
        canonical_json(case["value"])


@pytest.mark.parametrize(
    "value",
    [
        float("nan"),
        float("inf"),
        float("-inf"),
        "\ud800",
        {"\udfff": 1},
        {1: "key"},
        (1, 2),
        b"bytes",
    ],
    ids=[
        "nan",
        "infinity",
        "negative-infinity",
        "surrogate",
        "surrogate-key",
        "key-type",
        "tuple",
        "bytes",
    ],
)
def test_non_json_python_values(value: object) -> None:
    with pytest.raises(CanonicalError):
        canonical_json(value)


def test_byte_and_depth_limits_and_cycles() -> None:
    assert len(canonical_json("a" * (MAX_CANONICAL_BYTES - 2))) == MAX_CANONICAL_BYTES
    with pytest.raises(CanonicalError):
        canonical_json("é" * (MAX_CANONICAL_BYTES // 2))
    nested: object = None
    for _ in range(64):
        nested = [nested]
    canonical_json(nested)
    with pytest.raises(CanonicalError):
        canonical_json([nested])
    cycle: list[object] = []
    cycle.append(cycle)
    with pytest.raises(CanonicalError):
        canonical_json(cycle)


def test_raw_hash_vectors_short_reads_and_io_failure() -> None:
    class ShortReader(BytesIO):
        def read(self, size: int | None = -1, /) -> bytes:
            assert size == 64 * 1024
            return super().read(1)

    class BrokenReader(BytesIO):
        def read(self, size: int | None = -1, /) -> bytes:
            raise OSError("injected read error")

    for case in FIXTURES["files"]:
        digest = file_digest(ShortReader(case["text"].encode()))
        assert digest.hex == case["sha256"]
        assert digest.kind is DigestKind.FILE
    assert file_digest(BytesIO(b"a" * 1_000_000)).hex == (
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    )
    with pytest.raises(OSError):
        file_digest(BrokenReader())
    with pytest.raises(CanonicalError):
        content_digest(DigestKind.FILE, {})


def test_selected_semantics_preserve_user_parameters() -> None:
    for section, kind in [("source", DigestKind.SOURCE), ("semantic", DigestKind.COMPUTE)]:
        doc = deepcopy(FIXTURES[section]["document"])
        original = semantic_fingerprint(kind, doc)
        assert original.hex == FIXTURES[section]["sha256"]
        doc["presentation"] = {"description": "renamed", "position": [1, 2]}
        assert original == semantic_fingerprint(kind, doc)
        doc["semantic"]["description"] = "a computation parameter"
        assert original != semantic_fingerprint(kind, doc)
        doc["unclassified"] = True
        with pytest.raises(CanonicalError):
            semantic_fingerprint(kind, doc)
    original_doc = FIXTURES["semantic"]["document"]
    original = semantic_fingerprint(DigestKind.COMPUTE, original_doc)
    for field in ["alias", "version_id", "artifact_digest"]:
        changed = deepcopy(original_doc)
        changed["semantic"]["inputs"][0][field] = "changed"
        assert original != semantic_fingerprint(DigestKind.COMPUTE, changed)
    changed = deepcopy(original_doc)
    changed["semantic"]["parameters"]["description"] = "changed user data"
    assert original != semantic_fingerprint(DigestKind.COMPUTE, changed)
    changed = deepcopy(original_doc)
    changed["semantic"]["checks"][0]["severity"] = "WARN"
    assert original != semantic_fingerprint(DigestKind.COMPUTE, changed)
    with pytest.raises(CanonicalError):
        semantic_fingerprint(DigestKind.ARTIFACT, original_doc)


def test_artifact_schema_and_order_are_part_of_identity() -> None:
    manifest = deepcopy(FIXTURES["artifacts"]["manifest"])
    digest = artifact_digest(manifest)
    assert digest.hex == FIXTURES["artifacts"]["sha256"]
    manifest["files"].reverse()
    assert digest != artifact_digest(manifest)
    manifest["schema_fingerprint"] = "0" * 64
    with pytest.raises(CanonicalError):
        artifact_digest(manifest)
    manifest = deepcopy(FIXTURES["artifacts"]["manifest"])
    manifest["timestamp"] = "2026-09-19T00:00:00Z"
    with pytest.raises(CanonicalError):
        artifact_digest(manifest)
    publications = {
        "00000000-0000-4000-8000-000000000001": digest,
        "00000000-0000-4000-8000-000000000002": artifact_digest(FIXTURES["artifacts"]["manifest"]),
    }
    assert len(publications) == 2
    assert len(set(publications.values())) == 1


def test_catalogue_projection_and_tampering() -> None:
    snapshot = deepcopy(FIXTURES["catalog"]["snapshot"])
    verify_catalog_fingerprint(snapshot)
    original = catalog_fingerprint(snapshot)
    snapshot["source_snapshot_id"] = "00000000-0000-4000-8000-000000000002"
    assert original == catalog_fingerprint(snapshot)
    snapshot["entries"][0]["path"] = "raw/renamed"
    assert original != catalog_fingerprint(snapshot)
    with pytest.raises(CanonicalError):
        verify_catalog_fingerprint(snapshot)
