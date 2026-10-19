import { expect, test } from "vitest";
import { checkStatus, buildStatus } from "./health-model";
import { formatSize } from "./format-size";
const check = (outcome: string, severity = "FAIL") => ({
  outcome,
  policy: { severity },
});
test("health respects severity and never presents absent or incomplete checks as success", () => {
  expect(checkStatus([]).tone).toBe("neutral");
  expect(checkStatus([check("PASS")]).tone).toBe("good");
  expect(checkStatus([check("VIOLATION", "WARN")]).tone).toBe("warn");
  expect(
    checkStatus([check("VIOLATION", "WARN"), check("VIOLATION")]).tone,
  ).toBe("bad");
  expect(checkStatus([check("ERROR", "WARN")]).tone).toBe("bad");
  expect(checkStatus([check("SKIPPED")]).tone).toBe("neutral");
  expect(checkStatus([check("VIOLATION", "unknown")]).tone).toBe("neutral");
  expect(buildStatus("CACHED")).toEqual({ label: "Cached", tone: "good" });
  expect(buildStatus("RUNNING").tone).toBe("neutral");
});
test("size formatting uses decimal units without losing large integer precision", () => {
  expect(formatSize(undefined)).toBe("Unavailable");
  expect(formatSize("0")).toBe("0 KB");
  expect(formatSize("159")).toBe("0.16 KB");
  expect(formatSize("1000000")).toBe("1 MB");
  expect(formatSize("1000000000")).toBe("1 GB");
  expect(formatSize("1000000000000")).toBe("1 TB");
  expect(formatSize("9007199254740993")).toBe("9.01 PB");
});
