"""Test-only assertions for the documented schema subset and Transflow invariants.

This consumes the one Rust-owned schema; it is not an installed SDK validator.
"""

import base64
import binascii
import datetime
import math
import re
import struct
from typing import Any

KEYWORDS = {
    "$ref",
    "type",
    "const",
    "enum",
    "oneOf",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
    "format",
    "x-transflow-invariant",
}
KEYWORDS_PYTHON = set(
    "False None True and as assert async await break class continue def del elif else "
    "except finally for from global if import in is lambda nonlocal not or pass raise "
    "return try while with yield".split()
)


def need(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def number(value: object) -> bool:
    return type(value) in (int, float) and math.isfinite(value)  # type: ignore[arg-type]


def same(a: Any, b: Any) -> bool:
    return (type(a) is type(b) or (number(a) and number(b))) and a == b


def date(value: str) -> None:
    need(re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", value) is not None, "date syntax")
    need(datetime.date.fromisoformat(value).isoformat() == value, "date")


def check_format(fmt: str, value: str) -> None:
    name = fmt.removeprefix("transflow-")
    if name == "diagnostic-text":

        def forbidden(c: str) -> bool:
            n = ord(c)
            return (
                n < 32
                or 127 <= n <= 159
                or n in (0x61C, 0x200E, 0x200F, 0xFEFF)
                or 0x2028 <= n <= 0x202E
                or 0x2066 <= n <= 0x2069
            )

        need(
            bool(value)
            and len(value.encode("utf-8")) <= 32768
            and not any(forbidden(c) for c in value),
            name,
        )
        return
    if name == "uuid":
        need(
            re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", value)
            is not None,
            fmt,
        )
    elif name == "sha256":
        need(re.fullmatch(r"[0-9a-f]{64}", value) is not None, fmt)
    elif name in ("i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"):
        need(re.fullmatch(r"0|-?[1-9][0-9]*", value) is not None, fmt)
        bits = int(name[1:])
        low, high = (
            (-(2 ** (bits - 1)), 2 ** (bits - 1) - 1) if name[0] == "i" else (0, 2**bits - 1)
        )
        need(low <= int(value) <= high, fmt)
    elif name in ("f32", "f64"):
        if value in ("NaN", "Infinity", "-Infinity"):
            return
        need(
            re.fullmatch(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?", value) is not None, fmt
        )
        decoded = float(value)
        nonzero = any(c in "123456789" for c in re.split("[eE]", value)[0])
        need(math.isfinite(decoded) and (decoded != 0 or not nonzero), fmt)
        try:
            if name == "f32":
                rounded = struct.unpack("!f", struct.pack("!f", decoded))[0]
                need(math.isfinite(rounded) and (rounded != 0 or not nonzero), fmt)
        except OverflowError as exc:
            raise ValueError(fmt) from exc
    elif name == "date":
        date(value)
    elif name == "base64":
        try:
            decoded_bytes = base64.b64decode(value, validate=True)
            need(base64.b64encode(decoded_bytes).decode() == value, fmt)
        except (binascii.Error, UnicodeEncodeError) as exc:
            raise ValueError(fmt) from exc
    elif name == "relative-path":
        need(
            0 < len(value) <= 1024
            and "\\" not in value
            and ":" not in value
            and all(part not in ("", ".", "..") for part in value.split("/"))
            and all(ord(c) >= 32 and ord(c) != 127 for c in value),
            fmt,
        )
    elif name == "dataset-path":
        need(
            all(
                re.fullmatch(r"[a-z][a-z0-9_]*", part) and part not in KEYWORDS_PYTHON
                for part in value.split("/")
            ),
            fmt,
        )
    elif name == "name":
        need(0 < len(value) <= 256 and all(ord(c) >= 32 and ord(c) != 127 for c in value), fmt)
    elif name == "timezone":
        need(
            value == "UTC" or re.fullmatch(r"[A-Za-z_+-]+(/[A-Za-z0-9_+-]+)+", value) is not None,
            fmt,
        )
    else:
        raise ValueError("Unknown asserted format: " + fmt)


def invariant(name: str, value: dict[str, Any]) -> None:
    if name == "decimal-value":
        raw, scale = value["value"], int(value["scale"])
        need(re.fullmatch(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?", raw) is not None, name)
        unsigned = raw.removeprefix("-")
        if scale > 0:
            need("." in unsigned and len(unsigned.split(".")[1]) == scale, name)
            unscaled = unsigned.replace(".", "")
        else:
            need("." not in unsigned, name)
            need(scale == 0 or unsigned == "0" or unsigned.endswith("0" * -scale), name)
            unscaled = unsigned[:scale] if scale < 0 and unsigned != "0" else unsigned
        need(len(unscaled.lstrip("0") or "0") <= value["precision"], name)
    elif name == "timestamp-value":
        raw = value["value"]
        if value["timezone"] is not None:
            need(raw.endswith("Z"), name)
            raw = raw[:-1]
        need(len(raw) >= 19 and raw[10] == "T" and raw[13] == ":" and raw[16] == ":", name)
        date(raw[:10])
        for text, maximum in ((raw[11:13], 23), (raw[14:16], 59), (raw[17:19], 59)):
            need(re.fullmatch("[0-9]{2}", text) is not None and int(text) <= maximum, name)
        digits = {"s": 0, "ms": 3, "us": 6, "ns": 9}[value["unit"]]
        suffix = raw[19:]
        need(
            suffix == ""
            if digits == 0
            else re.fullmatch(r"\.[0-9]{" + str(digits) + "}", suffix) is not None,
            name,
        )
    elif name in ("field-names", "file-names"):
        items, key = (
            (value["fields"], "name") if name == "field-names" else (value["files"], "path")
        )
        need(len({item[key] for item in items}) == len(items), name)
    elif name == "catalog-entries":
        entries = value["entries"]
        need(len({e["path"] for e in entries}) == len(entries), name)
        need(
            len({(e["key"]["workspace_id"], e["key"]["dataset_id"]) for e in entries})
            == len(entries),
            name,
        )
        for entry in entries:
            need(entry["path"] != "external", name)
            foreign = entry["key"]["workspace_id"] != value["workspace_id"]
            need((entry["kind"] == "external") == foreign, name)
            need(entry["path"].startswith("external/") == foreign, name)
        aliases = value.get("aliases", [])
        paths = {e["path"] for e in entries}
        keys = {(e["key"]["workspace_id"], e["key"]["dataset_id"]) for e in entries}
        for alias in aliases:
            need(alias["path"] != "external", name)
            alias_key = (alias["key"]["workspace_id"], alias["key"]["dataset_id"])
            need(alias["path"] not in paths and alias_key in keys, name)
            need(
                alias["path"].startswith("external/") == (alias_key[0] != value["workspace_id"]),
                name,
            )
            paths.add(alias["path"])
    elif name == "frame-capabilities":
        required = value["required_capabilities"]
        extensions = value["extensions"]
        supported = {
            "diagnostic.note.v1",
            "discovery.v1",
            "polars.execute.v1",
            "expectation.ast.v1",
            "expectation.core.v1",
            "duckdb.checks.v1",
            "duckdb.samples.v1",
        }  # Fixture reader profile; not a live worker capability.
        need(len(set(required)) == len(required) and set(required) <= supported, name)
        need(len({e["capability"] for e in extensions}) == len(extensions), name)
        need(all(e["capability"] == "diagnostic.note.v1" for e in extensions), name)
        need(all(e["value"]["type"] == "string" for e in extensions), name)
    else:
        raise ValueError("Unknown invariant: " + name)


def validate(
    schema: dict[str, Any], value: Any, definitions: dict[str, Any], depth: int = 0
) -> None:
    need(depth <= 64, "fixture nesting limit")
    need(set(schema) <= KEYWORDS, "unsupported schema keyword")
    # Check discriminators before recursive children, regardless of JSON key order.
    if isinstance(value, dict):
        for key, rule in schema.get("properties", {}).items():
            if "const" in rule and key in value:
                need(same(value[key], rule["const"]), "const")
    if "$ref" in schema:
        validate(
            definitions[schema["$ref"].removeprefix("#/$defs/")], value, definitions, depth + 1
        )
        return
    if "oneOf" in schema:
        matches = 0
        for branch in schema["oneOf"]:
            try:
                validate(branch, value, definitions, depth + 1)
                matches += 1
            except ValueError:
                pass
        need(matches == 1, "oneOf")
    if "const" in schema:
        need(same(value, schema["const"]), "const")
    if "enum" in schema:
        need(any(same(value, item) for item in schema["enum"]), "enum")
    kind = schema.get("type")
    if kind == "string":
        need(
            isinstance(value, str) and all(not 0xD800 <= ord(c) <= 0xDFFF for c in value), "string"
        )
        if "format" in schema:
            check_format(schema["format"], value)
    elif kind == "integer":
        need(number(value) and int(value) == value, "integer")
        need(schema["minimum"] <= value <= schema["maximum"], "integer range")
    elif kind == "boolean":
        need(type(value) is bool, "boolean")
    elif kind == "null":
        need(value is None, "null")
    elif kind == "array":
        need(isinstance(value, list), "array")
        need(len(value) >= schema["minItems"], "array minimum")
        if "maxItems" in schema:
            need(len(value) <= schema["maxItems"], "array maximum")
        for item in value:
            validate(schema["items"], item, definitions, depth + 1)
    elif kind == "object":
        need(isinstance(value, dict), "object")
        props = schema.get("properties", {})
        need(all(key in value for key in schema.get("required", [])), "required")
        for key, item in value.items():
            if key in props:
                validate(props[key], item, definitions, depth + 1)
            else:
                extra = schema.get("additionalProperties", False)
                need(isinstance(extra, dict), "unknown field")
                validate(extra, item, definitions, depth + 1)
    elif kind is not None:
        raise ValueError("Unknown schema type")
    if "x-transflow-invariant" in schema:
        invariant(schema["x-transflow-invariant"], value)
