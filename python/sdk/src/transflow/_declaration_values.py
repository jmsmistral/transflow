"""Immutable copies of already versioned logical-schema and typed-value contracts."""

import json
from dataclasses import dataclass


class DeclarationError(ValueError):
    """An authored declaration is invalid before execution or catalogue mutation."""


def frozen_json(value: object, contract: str | None = None) -> bytes:
    # Lazy, pure generated-contract reuse; importing the SDK starts no worker/engine.
    try:
        raw = json.dumps(value, allow_nan=False, ensure_ascii=False, sort_keys=True).encode()
        if len(raw) > 1024 * 1024:
            raise ValueError("Declaration exceeds 1 MiB")
        copied = json.loads(raw)
        if contract is not None:
            from transflow_worker.wire import validate_document

            validate_document(contract, copied)
        return raw
    except (TypeError, ValueError, RecursionError) as exc:
        raise DeclarationError(
            f"Expected a finite JSON value matching {contract or 'metadata'}"
        ) from exc


@dataclass(frozen=True, slots=True, init=False)
class Parameter:
    """Typed parameter default using the shared lossless wire-value contract.

    Lists/maps require an explicit logical type; plan overrides are later work.
    """

    _type: bytes
    _default: bytes

    def __init__(self, logical_type: object, default: object) -> None:
        type_bytes = frozen_json(logical_type, "LogicalType")
        default_bytes = frozen_json(default, "WireValue")
        _matches(json.loads(type_bytes), json.loads(default_bytes))
        object.__setattr__(self, "_type", type_bytes)
        object.__setattr__(self, "_default", default_bytes)

    @property
    def logical_type(self) -> object:
        return json.loads(self._type)

    @property
    def default(self) -> object:
        return json.loads(self._default)


def _matches(logical: dict[str, object], value: dict[str, object]) -> None:
    kind = logical["type"]
    if kind != value["type"]:
        raise DeclarationError("Parameter default does not match its declared logical type")
    if kind == "binary":
        raise DeclarationError("Binary values are not supported for parameters")
    if kind in {"f32", "f64"} and value["value"] in {"NaN", "Infinity", "-Infinity"}:
        raise DeclarationError("Parameter floats must be finite")
    for field in ("precision", "scale", "unit", "timezone"):
        if field in logical and logical[field] != value.get(field):
            raise DeclarationError("Parameter default has incompatible type details")
    if kind == "list":
        element, entries = logical["element"], value["values"]
        if not isinstance(element, dict) or not isinstance(entries, list):
            raise DeclarationError("List parameters require a typed element field")
        for entry in entries:
            _field_matches(element, entry)
    if kind == "struct":
        fields, entries = logical["fields"], value["fields"]
        if not isinstance(fields, list) or not isinstance(entries, list):
            raise DeclarationError("Map parameters require typed fields")
        if [f["name"] for f in fields] != [f["name"] for f in entries]:
            raise DeclarationError("Map parameter keys must match the declared fields in order")
        for field, entry in zip(fields, entries, strict=True):
            _field_matches(field, entry["value"])


def _field_matches(field: dict[str, object], value: object) -> None:
    logical = field["logical_type"]
    if not isinstance(logical, dict) or not isinstance(value, dict):
        raise DeclarationError("Parameter field is malformed")
    if value["type"] == "null" and field["nullable"] is True:
        return
    _matches(logical, value)
