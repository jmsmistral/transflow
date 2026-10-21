import { expect, test } from "vitest";
import { sqlPresentation } from "./sql-presentation";

test("Python SQL margins disappear while continuation alignment and inner blank lines remain", () => {
  const retained =
    "\n            SELECT region,\n                   COUNT(*) AS count\n\n            FROM deliveries\n        ";
  expect(sqlPresentation(retained)).toBe(
    "SELECT region,\n       COUNT(*) AS count\n\nFROM deliveries",
  );
  expect(retained.startsWith("\n            SELECT")).toBe(true);
});

test("unindented SQL keeps its intentional indentation and tab margins are handled", () => {
  expect(sqlPresentation("SELECT *\n    FROM deliveries")).toBe(
    "SELECT *\n    FROM deliveries",
  );
  expect(sqlPresentation("\n\t\tSELECT *\n\t\tFROM deliveries\n")).toBe(
    "SELECT *\nFROM deliveries",
  );
  expect(sqlPresentation(" \n\t\n")).toBe("");
});
