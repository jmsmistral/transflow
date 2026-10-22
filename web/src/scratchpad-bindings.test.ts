import { expect, test } from "vitest";
import { Client } from "./api/client";
import { bindCatalogueSql } from "./scratchpad-bindings";
import {
  context,
  dataset,
  datasetId,
  fixtureTransport,
  json,
  versionId,
  workspaceId,
} from "./test-fixtures";

const selected = {
  alias: "dataset",
  dataset: datasetId,
  version: versionId,
  origin_workspace: workspaceId,
};
const page = (entries: unknown[], cursor: string | null = null) => ({
  entries,
  next_cursor: cursor,
  total: String(entries.length),
  schema: null,
  freshness: "unknown",
});

test("the selected quoted path retains its displayed version without a catalogue head lookup", async () => {
  const transport = fixtureTransport(() => {
    throw new Error("The selected version must not be resolved again");
  });
  const bound = await bindCatalogueSql(
    new Client(transport),
    'SELECT "raw/example".id FROM "raw/example" JOIN dataset d USING(id)',
    selected,
    context(),
    { branch: "master" },
    new AbortController().signal,
    "raw/example",
  );
  expect(bound.bindings).toEqual([selected]);
  expect(bound.sql).toBe(
    'SELECT "dataset".id FROM "dataset" JOIN dataset d USING(id)',
  );
  expect(transport).not.toHaveBeenCalled();
});

test("resolves exact catalogue paths with bounded pages, branch context and fallback versions", async () => {
  const ctx = context("review");
  const queries: Record<string, string>[] = [];
  const client = new Client(
    fixtureTransport((path, query) => {
      if (path !== "/api/v1/datasets") return;
      queries.push(query);
      return Promise.resolve(
        json(
          query.cursor
            ? page([
                {
                  ...dataset,
                  path: "curated/east",
                  head: { version: versionId, resolved_branch: "master" },
                },
              ])
            : page([{ ...dataset, path: "curated/east_archive" }], "next"),
          ctx,
        ),
      );
    }),
  );
  const bound = await bindCatalogueSql(
    client,
    'SELECT * FROM dataset JOIN "curated/east" e USING(id)',
    selected,
    ctx,
    { branch: "review", fallback: '["master"]' },
    new AbortController().signal,
  );
  expect(queries).toHaveLength(2);
  expect(queries[0]).toMatchObject({
    branch: "review",
    fallback: '["master"]',
    filter: "curated/east",
    context: ctx.fingerprint,
  });
  expect(bound.bindings).toEqual([
    selected,
    { ...selected, alias: "__tf_catalogue_1" },
  ]);
  expect(bound.fallbacks).toEqual(["curated/east from branch master"]);
  expect(bound.sql).toContain('JOIN "__tf_catalogue_1" e');
});

test.each([
  "missing",
  "unpublished",
  "tombstone",
  "repeated page",
  "changed context",
])("refuses %s without inventing a binding", async (kind) => {
  const client = new Client(
    fixtureTransport((path) => {
      if (path !== "/api/v1/datasets") return;
      return Promise.resolve(
        json(
          page(
            kind === "missing" || kind === "repeated page"
              ? []
              : [
                  {
                    ...dataset,
                    path: "curated/east",
                    tombstone: kind === "tombstone",
                    head: null,
                  },
                ],
            kind === "repeated page" ? "same" : null,
          ),
          kind === "changed context"
            ? { ...context(), fingerprint: "f".repeat(64) }
            : context(),
        ),
      );
    }),
  );
  await expect(
    bindCatalogueSql(
      client,
      'SELECT * FROM "curated/east"',
      selected,
      context(),
      { branch: "master" },
      new AbortController().signal,
    ),
  ).rejects.toThrow();
});

test("uses retained external publication identity without executing provider code", async () => {
  const foreign = "11111111-1111-4111-8111-111111111111";
  const paths: string[] = [];
  const client = new Client(
    fixtureTransport((path) => {
      paths.push(path);
      if (path === "/api/v1/datasets")
        return Promise.resolve(
          json(
            page([
              {
                ...dataset,
                path: "shared/source",
                workspace_id: foreign,
                origin: "external",
                kind: "external",
                head: null,
              },
            ]),
            context(),
          ),
        );
      if (path.endsWith("/inspection"))
        return Promise.resolve(
          json(
            {
              origin_workspace: foreign,
              dataset: datasetId,
              created_us: null,
              suggested_head: {
                version: versionId,
                branch: "master",
                published_us: "1",
                schema: { format_version: 1, fields: [] },
              },
            },
            context(),
          ),
        );
    }),
  );
  const result = await bindCatalogueSql(
    client,
    'SELECT * FROM "shared/source"',
    selected,
    context(),
    { branch: "master" },
    new AbortController().signal,
  );
  expect(result.bindings[1]).toEqual({
    ...selected,
    alias: "__tf_catalogue_1",
    origin_workspace: foreign,
  });
  expect(paths).toEqual([
    "/api/v1/datasets",
    `/api/v1/datasets/${datasetId}/inspection`,
  ]);
});
