import { expect, test } from "vitest";
import { catalogueSql, sqlCompletion } from "./sql-catalogue";

test("binds quoted table paths in joins, nested queries and comma lists, preserving values", () => {
  const source = `SELECT "curated/east".id, 'FROM "not/a/table"', "curated/east"
FROM dataset d JOIN "curated/east" ON d.id = "curated/east".id,
(SELECT * FROM "curated/west") w -- JOIN "not/a/table"
WHERE d.region = $$JOIN "not/a/table"$$`;
  const translated = catalogueSql(source);
  expect(translated.paths).toEqual([
    { path: "curated/east", alias: "__tf_catalogue_1" },
    { path: "curated/west", alias: "__tf_catalogue_2" },
  ]);
  expect(translated.sql).toContain('SELECT "__tf_catalogue_1".id');
  expect(translated.sql).toContain('\'FROM "not/a/table"\', "curated/east"');
  expect(translated.sql).toContain(
    'JOIN "__tf_catalogue_1" ON d.id = "__tf_catalogue_1".id',
  );
  expect(translated.sql).toContain('(SELECT * FROM "__tf_catalogue_2")');
  expect(translated.sql).toContain('-- JOIN "not/a/table"');
  expect(translated.sql).toContain('$$JOIN "not/a/table"$$');
});

test("preserves scoped CTEs, column names, quoted dataset alias and escaped literals", () => {
  expect(
    catalogueSql(`WITH /* comment */ "summary" AS (SELECT * FROM "curated/east")
SELECT 'it''s FROM "other"', E'it\\'s FROM "other"', "curated/east" FROM "summary" JOIN "dataset" USING(id)`),
  ).toEqual({
    paths: [{ path: "curated/east", alias: "__tf_catalogue_1" }],
    sql: `WITH /* comment */ "summary" AS (SELECT * FROM "__tf_catalogue_1")
SELECT 'it''s FROM "other"', E'it\\'s FROM "other"', "curated/east" FROM "summary" JOIN "dataset" USING(id)`,
  });
  const scoped = catalogueSql(
    `SELECT * FROM (WITH "curated/east" AS (SELECT * FROM dataset) SELECT * FROM "curated/east") a JOIN "curated/east" b USING(id)`,
  );
  expect(scoped.paths).toHaveLength(1);
  expect(scoped.sql).toContain(
    'FROM "curated/east") a JOIN "__tf_catalogue_1"',
  );
});

test("deduplicates repeated paths, avoids aliases in authored code and preserves qualified schemas", () => {
  const result = catalogueSql(
    'SELECT * FROM "raw/a" __tf_catalogue_1 JOIN "raw/a" b USING(id) JOIN "catalog"."table" c USING(id)',
  );
  expect(result.paths).toEqual([{ path: "raw/a", alias: "__tf_catalogue_2" }]);
  expect(result.sql).toContain('JOIN "__tf_catalogue_2" b');
  expect(result.sql).toContain('JOIN "catalog"."table"');
  expect(
    catalogueSql('SELECT EXTRACT(YEAR FROM "day") FROM dataset').paths,
  ).toEqual([]);
});

test("refuses incomplete quotes, oversized SQL and too many catalog bindings", () => {
  expect(() => catalogueSql('SELECT * FROM "raw/orders')).toThrow("Close");
  expect(() => catalogueSql('SELECT * FROM "raw/orders""')).toThrow("Close");
  expect(() => catalogueSql(" ".repeat(16385))).toThrow("16 KiB");
  expect(() =>
    catalogueSql(
      `SELECT * FROM ${Array.from({ length: 32 }, (_, i) => `"raw/${i}"`).join(", ")}`,
    ),
  ).toThrow("31");
});

test("offers completion only at relation positions, including partial quotes and middle edits", () => {
  for (const sql of [
    "SELECT * FROM ",
    "SELECT * FROM dataset JOIN ",
    "WITH s AS (SELECT * FROM ",
  ])
    expect(sqlCompletion(sql, sql.length)).toEqual({
      start: sql.length,
      end: sql.length,
      query: "",
    });
  for (const sql of [
    "SELECT ",
    "SELECT 'FROM ",
    "SELECT * FROM dataset -- JOIN ",
    "SELECT * FROM dataset /* JOIN ",
    "SELECT $$FROM ",
    "SELECT EXTRACT(YEAR FROM ",
  ])
    expect(sqlCompletion(sql, sql.length)).toBeNull();
  const sql = 'SELECT * FROM "curated/east" e';
  expect(sqlCompletion(sql, 22)).toEqual({
    start: 14,
    end: 28,
    query: "curated",
  });
  expect(sqlCompletion("SELECT * FROM cu", 16)).toEqual({
    start: 14,
    end: 16,
    query: "cu",
  });
});
