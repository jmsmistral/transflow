// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ContractFailure, obj, list, str, validate } from "./api/validate";
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

describe("union validation", () => {
  const tagged = (tag: string) => ({
    type: "object",
    properties: {
      kind: { const: tag },
      value: { type: "integer", minimum: 0, maximum: 10 },
    },
    required: ["kind", "value"],
    additionalProperties: false,
  });

  it("checks the complete matching branch, including fields and bounds", () => {
    const union = { oneOf: [tagged("first"), tagged("second")] };
    expect(() =>
      validate(union, { kind: "second", value: 10 }, {}),
    ).not.toThrow();
    for (const value of [
      { kind: "unknown", value: 1 },
      { value: 1 },
      { kind: "second" },
      { kind: "second", value: 11 },
      { kind: "second", value: "1" },
      { kind: "second", value: 1, extra: true },
      null,
      [],
    ])
      expect(() => validate(union, value, {})).toThrow(ContractFailure);
  });

  it("rejects overlapping tags and overlapping primitive branches", () => {
    expect(() =>
      validate(
        { oneOf: [tagged("same"), tagged("same")] },
        { kind: "same", value: 1 },
        {},
      ),
    ).toThrow("oneOf");
    expect(() =>
      validate(
        {
          oneOf: [
            { type: "number" },
            { type: "integer", minimum: 0, maximum: 10 },
          ],
        },
        1,
        {},
      ),
    ).toThrow("oneOf");
  });

  it("does not treat optional constant properties as required tags", () => {
    const optional = {
      ...tagged("optional"),
      properties: { ...tagged("optional").properties, forbidden: false },
      required: ["value"],
    };
    expect(() =>
      validate({ oneOf: [optional, { type: "null" }] }, { value: 1 }, {}),
    ).not.toThrow();
    expect(() =>
      validate(
        { oneOf: [optional, tagged("other")] },
        { kind: "other", value: 1 },
        {},
      ),
    ).not.toThrow();
  });

  it("preserves reference precedence at union and property boundaries", () => {
    const defs = { Text: { type: "string" }, Label: { const: "actual" } };
    expect(() =>
      validate(
        { oneOf: [{ $ref: "#/$defs/Text", type: "null" }] },
        "text",
        defs,
      ),
    ).not.toThrow();
    expect(() =>
      validate(
        {
          oneOf: [
            {
              type: "object",
              properties: {
                kind: { $ref: "#/$defs/Label", const: "ignored" },
              },
              required: ["kind"],
              additionalProperties: false,
            },
          ],
        },
        { kind: "actual" },
        defs,
      ),
    ).not.toThrow();
  });
});
