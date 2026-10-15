import { expect, test } from "vitest";
import { tabSeparatedText } from "./tabular-text";

test("TSV keeps headers, exact text values and empty result schemas", () => {
  expect(
    tabSeparatedText(["id", "label"], [["9007199254740993", "café"]]),
  ).toBe("id\tlabel\n9007199254740993\tcafé");
  expect(tabSeparatedText(["id", "label"], [])).toBe("id\tlabel");
});
test("TSV quotes tabs, quotes and newlines and protects spreadsheet formulas", () => {
  expect(
    tabSeparatedText(["=header", "label"], [["=cmd", 'one\ttwo\n"three"']]),
  ).toBe('\'=header\tlabel\n\'=cmd\t"one\ttwo\n""three"""');
});
