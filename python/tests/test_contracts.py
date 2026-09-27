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


def test_generated_openapi_examples_and_parameter_names() -> None:
    document = json.loads((ROOT / "generated/openapi-v1.json").read_text())
    assert document["paths"]["/api/v1/sessions/launch"]["post"]["security"] == [{"bearer": []}]
    for route in json.loads((ROOT / "api-routes-v1.json").read_text()):
        operation = document["paths"][route["path"]][route["method"].lower()]
        parameters = [(p["in"], p["name"]) for p in operation["parameters"]]
        assert len(parameters) == len(set(parameters))
        content = operation["responses"]["200"]["content"]
        if route["path"] == "/api/v1/events":
            event = json.loads(content["text/event-stream"]["example"].split("data: ")[1])
            validate(SCHEMA["$defs"]["ApiEventV1"], event, SCHEMA["$defs"])
            assert ("header", "Last-Event-ID") in parameters
            continue
        response = content["application/json"]["example"]
        validate(SCHEMA["$defs"][route["response"]], response["data"], SCHEMA["$defs"])
        if response["context"] is not None:
            validate(SCHEMA["$defs"]["ApiContextV1"], response["context"], SCHEMA["$defs"])
        if route["request"]:
            body = operation["requestBody"]["content"]["application/json"]["example"]
            validate(SCHEMA["$defs"][route["request"]], body, SCHEMA["$defs"])
        if route["mutation"]:
            assert ("header", "Idempotency-Key") in parameters
            assert ("header", "If-Match") in parameters
