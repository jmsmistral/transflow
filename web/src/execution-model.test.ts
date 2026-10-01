import { expect, test } from "vitest";
import {
  historyDate,
  historyDateRange,
  percent,
  position,
  seconds,
  sourceDiff,
  timestamp,
} from "./execution-model";

test("inclusive history dates include a whole UTC day and reject invalid ranges", () => {
  const leapDay = historyDateRange("2024-02-29", "2024-02-29");
  expect(leapDay).toEqual({
    from_us: "1709164800000000",
    to_us: "1709251200000000",
  });
  expect(historyDate("1709251200000000", true)).toBe("2024-02-29");
  const yearEnd = historyDateRange("2023-12-31", "2024-01-01");
  expect(yearEnd).toEqual({
    from_us: "1703980800000000",
    to_us: "1704153600000000",
  });
  for (const dates of [
    ["2023-02-29", "2023-03-01"],
    ["2024-02-30", "2024-03-01"],
    ["2024-03-01", "2024-02-29"],
    ["2024-2-1", "2024-02-02"],
    ["01/02/2024", "2024-02-02"],
    ["1969-12-31", "1970-01-01"],
    ["", "2024-02-29"],
  ] as const)
    expect(historyDateRange(dates[0], dates[1])).toBeNull();
});
test("retained counts and duration ratios preserve precision above the JS integer range", () => {
  expect(seconds("9007199254740993123")).toBe("9007199254.741 s");
  expect(seconds({ numerator: "10000001", denominator: "2" })).toBe("0.005 s");
  expect(seconds(null)).toBe("Unavailable");
  expect(seconds("0")).toBe("0.000 s");
  expect(percent({ numerator: "1", denominator: "3" })).toBe("33.33%");
  expect(percent(null)).toBe("Unavailable");
  expect(position(9007199254741000100n, 9007199254741000000n, 1000n)).toBe(10);
  expect(timestamp("1700000000123456")).toBe("2023-11-14 22:13:20");
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
