// @vitest-environment node
import { expect, it } from "vitest";
import type { ControlFrameV1, WireValue } from "./generated/contracts";

it("generated types preserve wide values and discriminated messages", () => {
  const cell = {
    type: "u64",
    value: "18446744073709551615",
  } satisfies WireValue;
  const frame = {
    protocol: { major: 1, minor: 0 },
    request_id: "00000000-0000-4000-8000-000000000001",
    attempt_id: "00000000-0000-4000-8000-000000000002",
    sequence: "0",
    required_capabilities: [],
    extensions: [],
    message: { type: "metric", name: "total", value: cell },
  } satisfies ControlFrameV1;
  expect(JSON.parse(JSON.stringify(frame))).toEqual(frame);
  const invalid: WireValue = {
    type: "u64",
    // @ts-expect-error Wide integer carriers must not become JS numbers.
    value: Number("18446744073709551615"),
  };
  expect(invalid.type).toBe("u64");
  // @ts-expect-error Required opcodes are a closed union.
  const unknown: ControlFrameV1["message"] = { type: "run_arbitrary_code" };
  expect(unknown.type).toBe("run_arbitrary_code");
});
