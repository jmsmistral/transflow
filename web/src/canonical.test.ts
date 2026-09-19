// @vitest-environment node
import { describe, expect, test } from "vitest";
import fixtures from "../../schemas/fixtures/canonical-v1.json";
import {
  canonicalJson,
  contentDigest,
  CanonicalError,
  MAX_CANONICAL_BYTES,
  type DigestKind,
} from "./canonical";

const kinds: DigestKind[] = [
  "artifact",
  "source",
  "compute",
  "catalog",
  "schema",
];
describe("canonical v1 fixed cross-language vectors", () => {
  test.each(fixtures.cases)("$name", async (fixture) => {
    expect(new TextDecoder().decode(canonicalJson(fixture.value))).toBe(
      fixture.canonical,
    );
    const hashes = await Promise.all(
      kinds.map((kind) => contentDigest(kind, fixture.value)),
    );
    for (const hash of hashes) expect(hash.hex).toBe(fixture.sha256[hash.kind]);
    expect(new Set(hashes.map((hash) => hash.hex)).size).toBe(5);
  });
  test.each(fixtures.invalid)("rejects $name", (fixture) => {
    expect(() => canonicalJson(fixture.value)).toThrow(CanonicalError);
  });
});
test("non-JSON JavaScript values and invalid Unicode fail closed", () => {
  for (const value of [
    undefined,
    NaN,
    Infinity,
    -Infinity,
    1n,
    new Date(),
    /test/,
    "\ud800",
    { "\udfff": 1 },
    [undefined],
    new Array(2),
    { [Symbol("key")]: 1 },
  ]) {
    expect(() => canonicalJson(value)).toThrow(CanonicalError);
  }
  let called = false;
  expect(() =>
    canonicalJson({
      get field() {
        called = true;
        return 1;
      },
    }),
  ).toThrow(CanonicalError);
  expect(called).toBe(false);
});
test("bounds count UTF-8 bytes and nesting including cycles", () => {
  expect(canonicalJson("a".repeat(MAX_CANONICAL_BYTES - 2)).length).toBe(
    MAX_CANONICAL_BYTES,
  );
  expect(() => canonicalJson("é".repeat(MAX_CANONICAL_BYTES / 2))).toThrow(
    CanonicalError,
  );
  let nested: unknown = null;
  for (let i = 0; i < 64; i += 1) nested = [nested];
  expect(() => canonicalJson(nested)).not.toThrow();
  expect(() => canonicalJson([nested])).toThrow(CanonicalError);
  const cycle: unknown[] = [];
  cycle.push(cycle);
  expect(() => canonicalJson(cycle)).toThrow(CanonicalError);
});

test("catalogue aliases participate in the shared fingerprint", async () => {
  const entry = fixtures.catalog_aliases;
  expect(new TextDecoder().decode(canonicalJson(entry.projection))).toBe(
    entry.canonical,
  );
  expect((await contentDigest("catalog", entry.projection)).hex).toBe(
    entry.sha256,
  );
});
