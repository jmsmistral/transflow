// @vitest-environment node
// Test-only reader of the Rust-owned schema. No frontend runtime validator is shipped.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

class ContractFailure extends Error {}
function need(condition: unknown, reason: string): asserts condition {
  if (!condition) throw new ContractFailure(reason);
}
function obj(value: unknown): Record<string, unknown> {
  need(
    typeof value === "object" && value !== null && !Array.isArray(value),
    "object",
  );
  return value as Record<string, unknown>;
}
function list(value: unknown): unknown[] {
  need(Array.isArray(value), "array");
  return value;
}
function str(value: unknown): string {
  need(typeof value === "string", "string");
  need(
    Array.from(value).every((c) => {
      const code = c.codePointAt(0) ?? 0;
      return code < 0xd800 || code > 0xdfff;
    }),
    "Unicode scalar text",
  );
  return value;
}
function num(value: unknown): number {
  need(typeof value === "number" && Number.isFinite(value), "number");
  return value;
}
function same(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (Array.isArray(left) && Array.isArray(right))
    return (
      left.length === right.length && left.every((v, i) => same(v, right[i]))
    );
  if (
    typeof left === "object" &&
    left !== null &&
    !Array.isArray(left) &&
    typeof right === "object" &&
    right !== null &&
    !Array.isArray(right)
  ) {
    const a = obj(left),
      b = obj(right);
    return (
      Object.keys(a).length === Object.keys(b).length &&
      Object.keys(a).every(
        (key) => Object.hasOwn(b, key) && same(a[key], b[key]),
      )
    );
  }
  return false;
}
function matches(pattern: string, value: string): boolean {
  return new RegExp(`^(?:${pattern})$`, "u").exec(value)?.[0] === value;
}
function validDate(value: string): void {
  need(matches("[0-9]{4}-[0-9]{2}-[0-9]{2}", value), "date syntax");
  const y = Number(value.slice(0, 4)),
    m = Number(value.slice(5, 7)),
    d = Number(value.slice(8));
  const leap = y % 4 === 0 && (y % 100 !== 0 || y % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  need(y > 0 && d > 0 && d <= (days[m - 1] ?? 0), "date range");
}
const pythonKeywords = new Set(
  "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield".split(
    " ",
  ),
);
function format(raw: string, value: string): void {
  const name = raw.replace(/^transflow-/u, "");
  if (name === "diagnostic-text") {
    need(
      value.length > 0 &&
        new TextEncoder().encode(value).length <= 32768 &&
        Array.from(value).every((ch) => {
          const n = ch.codePointAt(0) ?? 0;
          return (
            n >= 32 &&
            !(n >= 127 && n <= 159) &&
            ![0x61c, 0x200e, 0x200f, 0xfeff].includes(n) &&
            !(n >= 0x2028 && n <= 0x202e) &&
            !(n >= 0x2066 && n <= 0x2069)
          );
        }),
      name,
    );
  } else if (name === "uuid")
    need(
      matches(
        "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
        value,
      ),
      name,
    );
  else if (name === "sha256") need(matches("[0-9a-f]{64}", value), name);
  else if (
    ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"].includes(name)
  ) {
    need(matches("0|-?[1-9][0-9]*", value), name);
    const bits = BigInt(name.slice(1)),
      n = BigInt(value);
    const signed = name.startsWith("i");
    const low = signed ? -(1n << (bits - 1n)) : 0n;
    const high = signed ? (1n << (bits - 1n)) - 1n : (1n << bits) - 1n;
    need(n >= low && n <= high, name);
  } else if (name === "f32" || name === "f64") {
    if (["NaN", "Infinity", "-Infinity"].includes(value)) return;
    need(
      matches("-?(0|[1-9][0-9]*)(\\.[0-9]+)?([eE][+-]?[0-9]+)?", value),
      name,
    );
    const decoded = Number(value);
    const nonzero = /[1-9]/u.test(value.split(/[eE]/u)[0] ?? "");
    need(
      Number.isFinite(decoded) &&
        (decoded !== 0 || !nonzero) &&
        (name === "f64" ||
          (Number.isFinite(Math.fround(decoded)) &&
            (Math.fround(decoded) !== 0 || !nonzero))),
      name,
    );
  } else if (name === "date") validDate(value);
  else if (name === "base64")
    need(Buffer.from(value, "base64").toString("base64") === value, name);
  else if (name === "relative-path")
    need(
      Array.from(value).length <= 1024 &&
        !/[\\:]/u.test(value) &&
        Array.from(value).every((c) => c >= " " && c !== "\x7f") &&
        value.split("/").every((p) => !["", ".", ".."].includes(p)),
      name,
    );
  else if (name === "name")
    need(
      value.length > 0 &&
        Array.from(value).length <= 256 &&
        Array.from(value).every((c) => c >= " " && c !== "\x7f"),
      name,
    );
  else if (name === "dataset-path")
    need(
      value
        .split("/")
        .every((p) => matches("[a-z][a-z0-9_]*", p) && !pythonKeywords.has(p)),
      name,
    );
  else if (name === "timezone")
    need(
      value === "UTC" || matches("[A-Za-z_+-]+(/[A-Za-z0-9_+-]+)+", value),
      name,
    );
  else throw new ContractFailure(`Unknown asserted format: ${raw}`);
}
function invariant(name: string, value: Record<string, unknown>): void {
  if (name === "decimal-value") {
    const raw = str(value.value),
      scale = num(value.scale);
    need(matches("-?(0|[1-9][0-9]*)(\\.[0-9]+)?", raw), name);
    const unsigned = raw.replace(/^-/u, "");
    let unscaled: string;
    if (scale > 0) {
      need(
        unsigned.includes(".") && unsigned.split(".")[1]?.length === scale,
        name,
      );
      unscaled = unsigned.replace(".", "");
    } else {
      need(
        !unsigned.includes(".") &&
          (scale === 0 ||
            unsigned === "0" ||
            unsigned.endsWith("0".repeat(-scale))),
        name,
      );
      unscaled =
        scale < 0 && unsigned !== "0" ? unsigned.slice(0, scale) : unsigned;
    }
    need(
      (unscaled.replace(/^0+/u, "") || "0").length <= num(value.precision),
      name,
    );
  } else if (name === "timestamp-value") {
    let raw = str(value.value);
    if (value.timezone !== null) {
      need(raw.endsWith("Z"), name);
      raw = raw.slice(0, -1);
    }
    need(
      raw.length >= 19 && raw[10] === "T" && raw[13] === ":" && raw[16] === ":",
      name,
    );
    validDate(raw.slice(0, 10));
    for (const [part, maximum] of [
      [raw.slice(11, 13), 23],
      [raw.slice(14, 16), 59],
      [raw.slice(17, 19), 59],
    ] as const)
      need(matches("[0-9]{2}", part) && Number(part) <= maximum, name);
    const digits = ({ s: 0, ms: 3, us: 6, ns: 9 } as Record<string, number>)[
      str(value.unit)
    ];
    const suffix = raw.slice(19);
    need(
      digits === 0
        ? suffix === ""
        : matches(`\\.[0-9]{${String(digits)}}`, suffix),
      name,
    );
  } else if (name === "field-names" || name === "file-names") {
    const items = list(name === "field-names" ? value.fields : value.files);
    const key = name === "field-names" ? "name" : "path";
    need(new Set(items.map((i) => obj(i)[key])).size === items.length, name);
  } else if (name === "catalog-entries") {
    const entries = list(value.entries).map(obj);
    need(new Set(entries.map((e) => e.path)).size === entries.length, name);
    need(
      new Set(
        entries.map((e) =>
          JSON.stringify([obj(e.key).workspace_id, obj(e.key).dataset_id]),
        ),
      ).size === entries.length,
      name,
    );
    for (const entry of entries) {
      need(entry.path !== "external", name);
      const foreign = obj(entry.key).workspace_id !== value.workspace_id;
      need(
        (entry.kind === "external") === foreign &&
          str(entry.path).startsWith("external/") === foreign,
        name,
      );
    }
    const paths = new Set(entries.map((e) => e.path));
    const keys = new Set(
      entries.map((e) =>
        JSON.stringify([obj(e.key).workspace_id, obj(e.key).dataset_id]),
      ),
    );
    for (const alias of list(value.aliases ?? []).map(obj)) {
      need(alias.path !== "external", name);
      const key = obj(alias.key);
      need(
        !paths.has(alias.path) &&
          keys.has(JSON.stringify([key.workspace_id, key.dataset_id])),
        name,
      );
      need(
        str(alias.path).startsWith("external/") ===
          (key.workspace_id !== value.workspace_id),
        name,
      );
      paths.add(alias.path);
    }
  } else if (name === "frame-capabilities") {
    const required = list(value.required_capabilities).map(str),
      extensions = list(value.extensions).map(obj);
    need(
      new Set(required).size === required.length &&
        new Set(extensions.map((e) => e.capability)).size === extensions.length,
      name,
    );
    need(
      required.every(
        (c) =>
          c === "diagnostic.note.v1" ||
          c === "discovery.v1" ||
          c === "polars.execute.v1" ||
          c === "expectation.ast.v1" ||
          c === "expectation.core.v1" ||
          c === "duckdb.checks.v1" ||
          c === "duckdb.samples.v1",
      ) &&
        extensions.every(
          (e) =>
            e.capability === "diagnostic.note.v1" &&
            obj(e.value).type === "string",
        ),
      name,
    );
  } else throw new ContractFailure(`Unknown invariant: ${name}`);
}
const keywords = new Set([
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
]);
function validate(
  schema: Record<string, unknown>,
  value: unknown,
  defs: Record<string, unknown>,
  depth = 0,
): void {
  need(depth <= 64, "fixture nesting limit");
  need(
    Object.keys(schema).every((k) => keywords.has(k)),
    "unsupported schema keyword",
  );
  if ("$ref" in schema) {
    validate(
      obj(defs[str(schema.$ref).replace(/^#\/\$defs\//u, "")]),
      value,
      defs,
      depth + 1,
    );
    return;
  }
  if ("oneOf" in schema) {
    let count = 0;
    for (const branch of list(schema.oneOf)) {
      try {
        validate(obj(branch), value, defs, depth + 1);
        count++;
      } catch (error) {
        if (!(error instanceof ContractFailure)) throw error;
      }
    }
    need(count === 1, "oneOf");
  }
  if ("const" in schema) need(same(value, schema.const), "const");
  if ("enum" in schema)
    need(
      list(schema.enum).some((v) => v === value),
      "enum",
    );
  switch (schema.type) {
    case "string": {
      const s = str(value);
      if ("format" in schema) format(str(schema.format), s);
      break;
    }
    case "integer": {
      const n = num(value);
      need(
        Number.isInteger(n) &&
          n >= num(schema.minimum) &&
          n <= num(schema.maximum),
        "integer range",
      );
      break;
    }
    case "boolean":
      need(typeof value === "boolean", "boolean");
      break;
    case "null":
      need(value === null, "null");
      break;
    case "array": {
      const items = list(value);
      need(
        items.length >= num(schema.minItems) &&
          (schema.maxItems === undefined ||
            items.length <= num(schema.maxItems)),
        "array bounds",
      );
      for (const item of items)
        validate(obj(schema.items), item, defs, depth + 1);
      break;
    }
    case "object": {
      const object = obj(value),
        props = obj(schema.properties ?? {});
      need(
        list(schema.required ?? []).every((k) => Object.hasOwn(object, str(k))),
        "required",
      );
      for (const [key, item] of Object.entries(object)) {
        const rule = Object.hasOwn(props, key)
          ? props[key]
          : schema.additionalProperties;
        validate(obj(rule), item, defs, depth + 1);
      }
      break;
    }
    case undefined:
      break;
    default:
      throw new ContractFailure("Unknown schema type");
  }
  if ("x-transflow-invariant" in schema)
    invariant(str(schema["x-transflow-invariant"]), obj(value));
}
function read(name: string): unknown {
  return JSON.parse(
    readFileSync(new URL(`../../schemas/${name}`, import.meta.url), "utf8"),
  ) as unknown;
}
const schema = obj(read("contracts-v1.schema.json")),
  definitions = obj(schema.$defs);
const cases = list(read("fixtures/conformance.json")).map(obj);
describe("T012 shared contract fixtures", () => {
  for (const test of cases)
    it(str(test.name), () => {
      if (test.valid === true) {
        validate(obj(definitions[str(test.schema)]), test.value, definitions);
        expect(JSON.parse(JSON.stringify(test.value)) as unknown).toEqual(
          test.value,
        );
      } else
        expect(() =>
          validate(obj(definitions[str(test.schema)]), test.value, definitions),
        ).toThrow(ContractFailure);
    });
  const versions = obj(read("versions.json"));
  for (const test of list(read("fixtures/versions.json")).map(obj))
    it(`version ${str(test.format)} ${String(test.version)}`, () => {
      expect(versions[str(test.format)] === test.version).toBe(test.valid);
    });
  it("rejects unknown schema rules", () =>
    expect(() => validate({ newRequiredRule: true }, {}, {})).toThrow(
      "unsupported schema keyword",
    ));
});
