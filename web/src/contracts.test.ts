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
