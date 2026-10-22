import { expect, test } from "vitest";
import { previewQuery, previewCellFilter } from "./preview-query";
import type { LogicalSchemaV1 } from "./generated/contracts";

const schema: LogicalSchemaV1 = {
  format_version: 1,
  fields: [
    {
      name: 'A "quoted" name',
      logical_type: { type: "string" },
      nullable: true,
    },
    {
      name: "amount",
      logical_type: { type: "decimal", precision: 24, scale: 2 },
      nullable: true,
    },
    { name: "id", logical_type: { type: "u64" }, nullable: false },
  ],
};
test("preview SQL quotes only schema names and binds values without numeric precision loss", () => {
  const query = previewQuery(schema, {
    filters: [
      {
        column: 'A "quoted" name',
        kind: "include",
        value: "' OR TRUE; DROP TABLE dataset; --",
      },
      { column: "amount", kind: "exclude", value: "9007199254740993.01" },
      { column: "id", kind: "not_null" },
    ],
    sort: { column: "id", direction: "desc" },
  });
  expect(query).toEqual({
    sql: 'SELECT * FROM dataset WHERE "A ""quoted"" name" IS NOT DISTINCT FROM CAST(? AS VARCHAR) AND "amount" IS DISTINCT FROM CAST(? AS DECIMAL(24, 2)) AND "id" IS NOT NULL ORDER BY "id" DESC NULLS LAST',
    parameters: ["' OR TRUE; DROP TABLE dataset; --", "9007199254740993.01"],
  });
});
test("null-only filters bind no value and missing columns fail explicitly", () => {
  expect(
    previewQuery(schema, {
      filters: [{ column: "amount", kind: "null" }],
      sort: null,
    }),
  ).toEqual({
    sql: 'SELECT * FROM dataset WHERE "amount" IS NULL',
    parameters: [],
  });
  expect(() =>
    previewQuery(schema, {
      filters: [],
      sort: { column: "missing", direction: "asc" },
    }),
  ).toThrow("unavailable");
});

test("cell filters preserve exact scalars, binary bytes and null semantics", () => {
  expect(
    previewCellFilter(
      "amount",
      {
        value: {
          type: "decimal",
          value: "9007199254740993.01",
          precision: 24,
          scale: 2,
        },
        truncated: false,
      },
      "include",
    ),
  ).toEqual({
    column: "amount",
    kind: "include",
    value: "9007199254740993.01",
  });
  expect(
    previewCellFilter(
      "id",
      {
        value: { type: "u64", value: "18446744073709551615" },
        truncated: false,
      },
      "exclude",
    ),
  ).toEqual({ column: "id", kind: "exclude", value: "18446744073709551615" });
  expect(
    previewCellFilter(
      "flag",
      { value: { type: "bool", value: false }, truncated: false },
      "include",
    ),
  ).toEqual({ column: "flag", kind: "include", value: "false" });
  expect(
    previewCellFilter(
      "value",
      { value: { type: "string", value: "" }, truncated: false },
      "include",
    ),
  ).toEqual({ column: "value", kind: "include", value: "" });
  expect(
    previewCellFilter(
      "time",
      {
        value: {
          type: "timestamp",
          value: "2026-10-22T12:34:56.123456789",
          unit: "ns",
          timezone: null,
        },
        truncated: false,
      },
      "include",
    ),
  ).toEqual({
    column: "time",
    kind: "include",
    value: "2026-10-22T12:34:56.123456789",
  });
  expect(
    previewCellFilter(
      "bytes",
      { value: { type: "binary", value: "AP9h" }, truncated: false },
      "include",
    ),
  ).toEqual({ column: "bytes", kind: "include", value: "\\x00\\xff\\x61" });
  for (const kind of ["include", "exclude"] as const) {
    expect(
      previewCellFilter(
        "value",
        { value: { type: "null" }, truncated: false },
        kind,
      ),
    ).toEqual({
      column: "value",
      kind: kind === "include" ? "null" : "not_null",
    });
    expect(
      previewCellFilter("value", { value: null, truncated: false }, kind),
    ).toBeNull();
    expect(
      previewCellFilter(
        "value",
        { value: { type: "string", value: "partial" }, truncated: true },
        kind,
      ),
    ).toBeNull();
    expect(
      previewCellFilter(
        "value",
        { value: { type: "list", values: [] }, truncated: false },
        kind,
      ),
    ).toBeNull();
  }
});
