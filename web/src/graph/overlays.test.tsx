import { expect, test } from "vitest";
import { paint, numeric, roles } from "./overlays";
import type {
  ApiLineageNodeV1,
  ApiOverlayFactsV1,
} from "../generated/contracts";
const facts: ApiOverlayFactsV1 = {
  version: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  rows: "0",
  files: "1",
  bytes: "100",
  latest_attempt: "Failed",
  freshness: "unknown",
  reasons: ["METADATA_UNKNOWN: comparison evidence"],
  health: "NoChecks",
  input_failed: true,
  durations: {
    last_ns: null,
    median_ns: { numerator: "3000000000", denominator: "2" },
    mean_ns: { numerator: "4000000000", denominator: "3" },
    samples: "2",
    missing: "1",
  },
  roles: ["target", "trigger"],
};
const node: ApiLineageNodeV1 = {
  identity: "data:a",
  paths: ["data/example"],
  depth: "0",
  external: false,
  producer: true,
  resource_type: "polars_transform",
  publication: "published",
  parent_count: "0",
  child_count: "1",
  overlay: facts,
};
test("failed latest attempt retains valid head and original output health independently of consumer failure", () => {
  expect(paint(node, "status")).toMatchObject({
    key: "Failed",
    badges: ["Published data available", "Input check failed"],
  });
  expect(paint(node, "health")).toMatchObject({
    key: "NoChecks",
    badges: ["Input check failed"],
  });
  expect(paint(node, "roles").badges).toEqual([
    "Input check failed",
    "Target",
    "Trigger",
  ]);
  expect(roles(node, null)).toEqual(["target", "trigger"]);
  expect(paint(node, "freshness")).toMatchObject({
    key: "unknown",
    detail: "METADATA_UNKNOWN: comparison evidence",
  });
});
test("numeric bins preserve exact zero and large counts and half-open thresholds", () => {
  expect(numeric(null, "rows").key).toBe("unknown");
  expect(numeric("0", "rows").key).toBe("zero");
  expect(numeric("999", "rows").key).toBe("bin0");
  expect(numeric("1000", "rows").key).toBe("bin1");
  expect(numeric("18446744073709551615", "rows").detail).toContain(
    "18,446,744,073,709,551,615",
  );
  expect(numeric("1048576", "bytes").key).toBe("bin1");
  expect(numeric("60000000000", "duration").key).toBe("bin1");
  expect(paint(node, "duration").key).toBe("unknown");
  expect(paint(node, "duration_median").detail).toBe("1,500 ms");
});
