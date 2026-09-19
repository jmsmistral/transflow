"""Cross-language fixtures for the authoritative T012 schema, not SDK API tests."""

import json
from pathlib import Path
from typing import Any

import pytest
from contracts.assertions import validate

ROOT = Path(__file__).resolve().parents[2] / "schemas"
SCHEMA = json.loads((ROOT / "contracts-v1.schema.json").read_text())
CASES = json.loads((ROOT / "fixtures/conformance.json").read_text())
VERSIONS = json.loads((ROOT / "versions.json").read_text())


@pytest.mark.parametrize("case", CASES, ids=[c["name"] for c in CASES])
def test_shared_contract(case: dict[str, Any]) -> None:
    schema = SCHEMA["$defs"][case["schema"]]
    if case["valid"]:
        validate(schema, case["value"], SCHEMA["$defs"])
        encoded = json.dumps(case["value"], ensure_ascii=False, allow_nan=False)
        assert json.loads(encoded) == case["value"]
    else:
        with pytest.raises(ValueError):
            validate(schema, case["value"], SCHEMA["$defs"])


@pytest.mark.parametrize("case", json.loads((ROOT / "fixtures/versions.json").read_text()))
def test_independent_versions(case: dict[str, Any]) -> None:
    assert (VERSIONS.get(case["format"]) == case["version"]) == case["valid"]


def test_unknown_schema_rule_fails_closed() -> None:
    with pytest.raises(ValueError, match="unsupported schema keyword"):
        validate({"newRequiredRule": True}, {}, {})
