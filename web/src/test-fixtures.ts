import { vi } from "vitest";
import type { ApiContextV1 } from "./generated/contracts";
export const workspaceId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
export const datasetId = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
export const versionId = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
export const context = (branch = "master"): ApiContextV1 => ({
  workspace: workspaceId,
  branch,
  source: workspaceId,
  registry: "a".repeat(64),
  runtime_revision: "1",
  configuration: "b".repeat(64),
  selection: { kind: "retained_current", id: null, digest: null },
  graph: "c".repeat(64),
  freshness: "unknown",
  fingerprint: (branch === "master" ? "d" : "e").repeat(64),
  fallback_policy: [branch, "public"],
});
export const dataset = {
  workspace_id: workspaceId,
  dataset_id: datasetId,
  path: "raw/example",
  kind: "source",
  origin: "local",
  tombstone: false,
  aliases: [],
  alias_count: "0",
  producer: null,
  tags: [],
  head: { version: versionId, resolved_branch: "public" },
  freshness: "unknown",
};
export const capabilities = {
  api_version: 1,
  protocol_major: 1,
  workspace: workspaceId,
  operations: [],
  engines: [],
  limits: {
    body_bytes: 2097152,
    in_flight: 16,
    page_default: 50,
    page_max: 200,
    graph_default: 100,
    graph_max: 500,
    graph_expansion_threshold: 500,
    source_bytes: 65536,
  },
  metadata_reads_import_code: false,
  external_reads: "provider_owned_leased",
  schedules: false,
  ui: true,
};
export function json(data: unknown, ctx: unknown = null): Response {
  return new Response(
    JSON.stringify({ request_id: workspaceId, context: ctx, data }),
    { headers: { "content-type": "application/json" } },
  );
}
export function fixtureTransport(
  override?: (
    path: string,
    query: Record<string, string>,
    signal: AbortSignal | null | undefined,
  ) => Promise<Response> | undefined,
): typeof fetch {
  return vi.fn(async (_url, init) => {
    const body = JSON.parse(String(init?.body)) as {
      path?: string;
      query?: Record<string, string>;
    };
    const url = String(_url);
    if (url.endsWith("/exchange"))
      return json({ csrf: "a".repeat(64), expires_in_seconds: 43200 });
    if (url.endsWith("/verify")) return json({ authenticated: true });
    const path = body.path ?? "",
      query = body.query ?? {};
    const special = override?.(path, query, init?.signal);
    if (special) return special;
    if (path.endsWith("/lineage") && query.connections)
      return json(
        {
          nodes: [],
          edges: [],
          next_cursor: null,
          total_nodes: 0,
          total_edges: 0,
          omitted_nodes: 0,
          omitted_edges: 0,
          remaining_nodes: 0,
          remaining_edges: 0,
          scope_complete: true,
          external_expanded: false,
        },
        context(query.branch),
      );
    if (path.endsWith("/capabilities")) return json(capabilities);
    if (path.endsWith("/events"))
      return new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener(
          "abort",
          () => reject(new Error("aborted")),
          { once: true },
        );
      });
    const base = context(query.branch);
    const c = query.version
      ? {
          ...base,
          selection: {
            kind: "local_version",
            id: query.version,
            digest: "f".repeat(64),
          },
          fingerprint: "f".repeat(64),
        }
      : base;
    if (path.endsWith("/context")) return json(c, c);
    if (path.endsWith("/datasets"))
      return json(
        {
          entries: [dataset],
          total: "1",
          next_cursor: null,
          schema: null,
          freshness: "unknown",
        },
        c,
      );
    if (path.endsWith("/branches"))
      return json(
        {
          entries: [{ name: "master" }, { name: "feature" }],
          next_cursor: null,
        },
        c,
      );
    if (path.endsWith("/versions"))
      return json({ entries: [], next_cursor: null }, c);
    return json(query.version ? { ...dataset, head: null } : dataset, c);
  });
}
