"""Deterministic contract exports from tf-protocol's authored schema (stdlib only)."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def typescript(node):
    if "$ref" in node:
        return node["$ref"].removeprefix("#/$defs/")
    if "oneOf" in node:
        return "(" + " | ".join(typescript(branch) for branch in node["oneOf"]) + ")"
    if "const" in node:
        return json.dumps(node["const"])
    if "enum" in node:
        return "(" + " | ".join(json.dumps(v) for v in node["enum"]) + ")"
    kind = node["type"]
    if kind in ("integer", "number"):
        return "number"
    if kind in ("boolean", "string", "null"):
        return kind
    if kind == "array":
        return "ReadonlyArray<" + typescript(node["items"]) + ">"
    if kind == "object":
        if isinstance(node.get("additionalProperties"), dict):
            # Index signatures support recursive JSON aliases; Record aliases do not.
            return "{ readonly [key: string]: " + typescript(node["additionalProperties"]) + " }"
        required = node.get("required", [])
        fields = [
            "readonly " + json.dumps(key) + ("" if key in required else "?") + ": " + typescript(value)
            for key, value in node.get("properties", {}).items()
        ]
        if not fields and node.get("additionalProperties") is False:
            return "Readonly<Record<string, never>>"
        return "{ " + "; ".join(fields) + " }"
    raise ValueError("Unsupported schema type: " + kind)


def write(relative, text):
    path = ROOT / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def example(node, definitions):
    if "$ref" in node:
        return example(definitions[node["$ref"].removeprefix("#/$defs/")], definitions)
    if "const" in node:
        return node["const"]
    if "enum" in node:
        return node["enum"][0]
    if "oneOf" in node:
        return example(node["oneOf"][0], definitions)
    kind = node["type"]
    if kind == "object":
        return {key: example(node["properties"][key], definitions) for key in node.get("required", [])}
    if kind == "array":
        return [example(node["items"], definitions) for _ in range(node.get("minItems", 0))]
    if kind == "string":
        formats = {"transflow-uuid": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "transflow-sha256": "a" * 64,
                   "transflow-u64": "0", "transflow-relative-path": "raw/example"}
        return formats.get(node.get("format"), "x" * node["minLength"] if node.get("minLength") else "example")
    return {"integer": node.get("minimum", 0), "number": 0, "boolean": False, "null": None}[kind]


def openapi(source):
    routes = json.loads((ROOT / "schemas/api-routes-v1.json").read_text())
    document = {"openapi": "3.1.0", "info": {"title": "Transflow loopback API", "version": "1"},
                "security": [{"bearer": []}, {"session": []}], "paths": {},
                "components": {"schemas": source["$defs"], "securitySchemes": {
                    "bearer": {"type": "http", "scheme": "bearer"},
                    "session": {"type": "apiKey", "in": "cookie", "name": "tf_session"}}}}
    for route in routes:
        schema = {"type": "object", "required": ["request_id", "context", "data"], "additionalProperties": False,
                  "properties": {"request_id": {"$ref": "#/$defs/Uuid"},
                                 "context": {"oneOf": [{"type": "null"}, {"$ref": "#/$defs/ApiContextV1"}]},
                                 "data": {"$ref": "#/$defs/" + route["response"]}}}
        operation = {"operationId": route["method"].lower() + "_" + route["path"].strip("/").replace("/", "_").replace("{", "").replace("}", ""),
                     "responses": {"200": {"description": "Context-bound result", "content": {"application/json": {"schema": schema}}},
                                   "default": {"description": "Typed safe error", "content": {"application/json": {"schema": {"$ref": "#/$defs/ApiErrorV1"}}}}},
                     "parameters": []}
        if route["path"] == "/health" or route["path"].endswith("/exchange"):
            operation["security"] = []
        elif route["path"] == "/api/v1/sessions/launch":
            operation["security"] = [{"bearer": []}]
        for name in dict.fromkeys((['branch', 'plan', 'context', 'version', 'dataset', 'origin_workspace'] if route['context'] else []) + route['query']):
            operation['parameters'].append({"name": name, "in": "query", "required": name == "branch", "schema": {"type": "string"}})
        for segment in route['path'].split('/'):
            if segment.startswith('{'):
                operation['parameters'].append({"name": segment[1:-1], "in": "path", "required": True, "schema": {"$ref": "#/$defs/Uuid"}})
        if route['request']:
            operation['requestBody'] = {"required": True, "content": {"application/json": {"schema": {"$ref": "#/$defs/" + route['request']}}}}
            operation['parameters'].append({"name": "X-Transflow-CSRF", "in": "header", "required": False, "description": "Required for browser session POSTs, except initial exchange", "schema": {"type": "string"}})
        if route['mutation']:
            for name in ['Idempotency-Key', 'If-Match']:
                operation['parameters'].append({"name": name, "in": "header", "required": True, "schema": {"type": "string"}})
        operation['responses']['200']['content']['application/json']['example'] = {
            "request_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "context": example(source['$defs']['ApiContextV1'], source['$defs']) if route['context'] else None,
            "data": example(source['$defs'][route['response']], source['$defs'])}
        if route['request']:
            operation['requestBody']['content']['application/json']['example'] = example(source['$defs'][route['request']], source['$defs'])
        document['paths'].setdefault(route['path'], {})[route['method'].lower()] = operation
    encoded = json.dumps(document, indent=2).replace('#/$defs/', '#/components/schemas/')
    write('schemas/generated/openapi-v1.json', encoded + '\n')


def main():
    source = json.loads((ROOT / "schemas/contracts-v1.schema.json").read_text())
    versions = json.loads((ROOT / "schemas/versions.json").read_text())
    exported = {**source, "$id": "urn:transflow:worker-control:v1", "$ref": "#/$defs/ControlFrameV1"}
    write("schemas/generated/worker-control-v1.schema.json", json.dumps(exported, indent=2) + "\n")
    cli = {**source, "$id": "urn:transflow:cli-result:v1", "$ref": "#/$defs/CliEnvelopeV1"}
    write("schemas/generated/cli-result-v1.schema.json", json.dumps(cli, indent=2) + "\n")
    openapi(source)
    ts = "// Generated by tools/contracts/generate.py. Do not edit.\n"
    for name, definition in source["$defs"].items():
        ts += "// prettier-ignore\nexport type " + name + " = " + typescript(definition) + ";\n\n"
    write("web/src/generated/contracts.ts", ts.rstrip() + "\n")
    # Embedded JSON remains data; it is not executable schema interpolation.
    schema_text = json.dumps(source, separators=(",", ":"))
    py = '"""Generated contract data; edit schemas/ and rerun the generator."""\n\n'
    py += "# fmt: off\n# ruff: noqa: E501\n"
    py += "SCHEMA_TEXT = " + repr(schema_text) + "\n"
    py += "PROTOCOL_MAJOR = " + str(versions["worker_protocol"]["major"]) + "\n"
    py += "PROTOCOL_MINOR = " + str(versions["worker_protocol"]["minor"]) + "\n"
    write("python/worker/src/transflow_worker/_wire_schema.py", py)
    validators = '"""Generated named validators backed by the shared runtime assertion engine."""\n\n'
    validators += "from .wire import validate_document\n\n"
    for name in source["$defs"]:
        validators += '\ndef validate_' + name + '(value: object) -> None:\n'
        validators += '    """Assert the ' + name + ' contract, including custom formats."""\n'
        validators += '    validate_document("' + name + '", value)\n\n'
    write("python/worker/src/transflow_worker/_wire_validators.py", validators.rstrip() + "\n")


if __name__ == "__main__":
    main()
