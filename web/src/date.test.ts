import { expect, test } from "vitest";
import { formatTimestamp } from "./date";
test("timestamps use YYYY-MM-DD and local time, preserving epoch and unknown values", () => {
  const timestamp = new Date(2026, 0, 2, 3, 4, 5).getTime();
  expect(formatTimestamp(String(BigInt(timestamp) * 1000n))).toBe(
    "2026-01-02 03:04:05",
  );
  expect(formatTimestamp("0")).not.toBe("Not available");
  for (const invalid of [
    null,
    undefined,
    {},
    "bad",
    "999999999999999999999999999999999999",
  ])
    expect(formatTimestamp(invalid)).toBe("Not available");
});
