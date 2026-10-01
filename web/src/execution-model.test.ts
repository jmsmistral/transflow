import { expect, test } from "vitest";
import {
  percent,
  position,
  seconds,
  sourceDiff,
  timestamp,
} from "./execution-model";

test("retained counts and duration ratios preserve precision above the JS integer range", () => {
  expect(seconds("9007199254740993123")).toBe("9007199254.741 s");
  expect(seconds({ numerator: "10000001", denominator: "2" })).toBe("0.005 s");
  expect(seconds(null)).toBe("Unavailable");
  expect(seconds("0")).toBe("0.000 s");
  expect(percent({ numerator: "1", denominator: "3" })).toBe("33.33%");
  expect(percent(null)).toBe("Unavailable");
  expect(position(9007199254741000100n, 9007199254741000000n, 1000n)).toBe(10);
  expect(timestamp("1700000000123456")).toBe("2023-11-14 22:13:20 UTC");
});
test("source diff preserves both complete captures, including insertion and identical files", () => {
  expect(sourceDiff("a\nb\nc", "a\nx\nc")).toEqual([
    { kind: "same", text: "a" },
    { kind: "removed", text: "b" },
    { kind: "added", text: "x" },
    { kind: "same", text: "c" },
  ]);
  expect(sourceDiff("a", "a")).toEqual([{ kind: "same", text: "a" }]);
  expect(sourceDiff("a", "a\nb")).toEqual([
    { kind: "same", text: "a" },
    { kind: "added", text: "b" },
  ]);
});
