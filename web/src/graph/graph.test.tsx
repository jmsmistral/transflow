import { render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Client } from "../api/client";
import type {
  ApiLineageV1,
  GraphEdgeV1,
  ApiLineageNodeV1,
} from "../generated/contracts";
import { context, fixtureTransport, json } from "../test-fixtures";
import { Workspace } from "../workspace";
import { GraphModel, depthQuery } from "./model";
import { Positions, type LayoutWorker } from "./positions";
import { layoutGraph, type LayoutReply } from "./layout";
import { GraphExplorer } from "./Graph";
const node = (id: string, external = false): ApiLineageNodeV1 => ({
  identity: id,
  paths: [`data/${id}`],
  depth: "0",
  external,
  producer: !external,
  parent_count: external ? "0" : "1",
  child_count: external ? "1" : "0",
});
const edge = (
  parent: string,
  consumer: string,
  alias = parent,
  role: GraphEdgeV1["role"] = "data",
): GraphEdgeV1 => ({
  parent,
  consumer,
  alias,
  role,
  declared_branch: { kind: "omitted", name: null },
  stop_branch_fallback: false,
  checks: [],
});
const page = (
  nodes: readonly ApiLineageNodeV1[],
  edges: readonly GraphEdgeV1[] = [],
  extra: Partial<ApiLineageV1> = {},
): ApiLineageV1 => ({
  nodes,
  edges,
  total_nodes: nodes.length,
  total_edges: edges.length,
  next_cursor: null,
  remaining_nodes: 0,
  remaining_edges: 0,
  omitted_nodes: 0,
  omitted_edges: 0,
  scope_complete: true,
  external_expanded: false,
  ...extra,
});
const query = { start: "data/a", direction: "downstream", depth: "" } as const;

test("depth zero, unlimited and invalid depths keep distinct semantics", () => {
  expect(depthQuery("")).toEqual({});
  expect(depthQuery("0")).toEqual({ depth: "0" });
  for (const invalid of ["-1", "1.5", " 1", "+1", "18446744073709551616"])
    expect(() => depthQuery(invalid)).toThrow();
});
test("diamond pages deduplicate shared nodes, retain aliases/roles and pending endpoints", async () => {
  const calls: Record<string, string>[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        calls.push(q);
        return Promise.resolve(
          json(
            q.cursor
              ? page(
                  [node("a"), node("b"), node("c")],
                  [edge("a", "c", "check", "validation")],
                )
              : page([node("a")], [edge("a", "b")], {
                  next_cursor: "next",
                  total_nodes: 3,
                  remaining_nodes: 2,
                  omitted_nodes: 7,
                  scope_complete: false,
                }),
            context(),
          ),
        );
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const stop = model.connect();
  await model.explore(query);
  expect(calls).toHaveLength(1);
  expect(calls[0]?.limit).toBe("100");
  expect(calls[0]).not.toHaveProperty("depth");
  expect(model.snapshot().page?.remaining_nodes).toBe(2);
  expect(model.snapshot().page?.omitted_nodes).toBe(7);
  await model.explore(query, true);
  expect(model.snapshot().nodes.map((n) => n.identity)).toEqual([
    "a",
    "b",
    "c",
  ]);
  expect(model.snapshot().edges.map((e) => e.role)).toEqual([
    "data",
    "validation",
  ]);
  model.remove(new Set(["a"]));
  expect(model.snapshot().edges).toEqual([]);
  expect(model.snapshot().page).toBeNull();
  stop();
  workspace.dispose();
});
test("500-node guard pauses presentation without silently discarding continuation", async () => {
  let batch = 0;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) => {
        if (!path.endsWith("/lineage")) return;
        const start = batch++ * 100;
        return Promise.resolve(
          json(
            page(
              Array.from({ length: 100 }, (_, i) => node(String(start + i))),
              [],
              {
                next_cursor: `batch${batch}`,
                total_nodes: 700,
                remaining_nodes: 700 - start - 100,
              },
            ),
            context(),
          ),
        );
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const stop = model.connect();
  await model.explore(query);
  for (let i = 1; i < 5; i++) await model.explore(query, true);
  const cursor = model.snapshot().page?.next_cursor;
  await model.explore(query, true);
  expect(model.snapshot().nodes).toHaveLength(500);
  expect(model.snapshot().page?.next_cursor).toBe(cursor);
  expect(model.snapshot().message).toContain("500");
  batch--;
  await model.explore(query, true, true);
  expect(model.snapshot().nodes).toHaveLength(600);
  stop();
  workspace.dispose();
});
test("new query and branch changes cancel reads and ignore late responses", async () => {
  let release: (r: Response) => void = () => undefined;
  let signal: AbortSignal | null | undefined;
  let slow = true;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q, s) => {
        if (!path.endsWith("/lineage")) return;
        if (slow) {
          signal = s;
          return new Promise<Response>((r) => {
            release = r;
          });
        }
        return Promise.resolve(json(page([node("new")]), context(q.branch)));
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const stop = model.connect();
  const first = model.explore(query);
  await vi.waitFor(() => expect(signal).toBeTruthy());
  slow = false;
  await model.explore({ ...query, start: "data/new" });
  expect(signal?.aborted).toBe(true);
  release(json(page([node("old")]), context()));
  await first;
  expect(model.snapshot().nodes[0]?.identity).toBe("new");
  slow = true;
  const second = model.explore(query);
  await workspace.select({ branch: "feature" });
  release(json(page([node("old")]), context()));
  await second;
  expect(model.snapshot().context).toBe(context("feature").fingerprint);
  expect(model.snapshot().nodes).toEqual([]);
  stop();
  workspace.dispose();
});
test("failed expansion retains prior view and path requests are server-scoped", async () => {
  let fail = false;
  const queries: Record<string, string>[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        queries.push(q);
        return Promise.resolve(
          fail
            ? new Response("", { status: 409 })
            : json(page([node("foreign", true)]), context()),
        );
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const stop = model.connect();
  await model.explore({ ...query, end: "data/end", depth: "2" });
  expect(queries[0]?.end).toBe("data/end");
  expect([...(model.snapshot().pathNodes ?? [])]).toEqual(["foreign"]);
  expect(queries[0]).not.toHaveProperty("depth");
  fail = true;
  await model.explore(query);
  expect(model.snapshot().nodes).toHaveLength(1);
  expect(model.snapshot().message).toContain("context changed");
  expect(queries.every((q) => q.limit === "100")).toBe(true);
  stop();
  workspace.dispose();
});
function fakeWorker(): LayoutWorker & { sent: unknown[]; terminated: boolean } {
  return {
    onmessage: null,
    onerror: null,
    sent: [],
    terminated: false,
    postMessage(value) {
      this.sent.push(value);
    },
    terminate() {
      this.terminated = true;
    },
  };
}
test("layout runs off-thread with deterministic IDs; pins and stale worker fencing survive expansion", () => {
  const workers: ReturnType<typeof fakeWorker>[] = [];
  const positions = new Positions(() => {
    const worker = fakeWorker();
    workers.push(worker);
    return worker;
  });
  const nodes = [node("b"), node("a")];
  expect(layoutGraph(nodes, [edge("a", "b"), edge("missing", "b")])).toEqual(
    layoutGraph([...nodes].reverse(), [edge("a", "b")]),
  );
  positions.run(nodes, []);
  const first = workers[0];
  expect(first?.sent).toHaveLength(1);
  positions.move("a", { x: 900, y: 700 });
  first?.onmessage?.(
    new MessageEvent<LayoutReply>("message", {
      data: { id: 1, positions: { a: { x: 0, y: 0 }, b: { x: 300, y: 0 } } },
    }),
  );
  expect(positions.snapshot().positions.a).toEqual({ x: 900, y: 700 });
  positions.run(nodes, []);
  expect(first?.terminated).toBe(true);
  first?.onmessage?.(
    new MessageEvent<LayoutReply>("message", {
      data: { id: 1, positions: { b: { x: 999, y: 999 } } },
    }),
  );
  expect(positions.snapshot().positions.b).toEqual({ x: 300, y: 0 });
  workers[1]?.onerror?.(new ErrorEvent("error"));
  expect(positions.snapshot().error).toContain("worker failed");
  expect(positions.snapshot().pins.has("a")).toBe(true);
  positions.run(nodes, [], true);
  expect(positions.snapshot().pins.size).toBe(0);
  positions.stop();
});
test("unavailable worker leaves a recoverable error and no invented layout success", () => {
  const positions = new Positions(() => {
    throw new Error("unavailable");
  });
  positions.run([node("a")], []);
  expect(positions.snapshot().busy).toBe(false);
  expect(positions.snapshot().error).toContain("unavailable");
});
test("graph exposes a diagram without the removed dataset list", async () => {
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  render(<GraphExplorer workspace={workspace} />);
  expect(
    screen.queryByRole("button", { name: "Show dataset list" }),
  ).toBeNull();
  expect(
    screen.getByRole("application", { name: "Dataset lineage diagram" }),
  ).toBeTruthy();
  workspace.dispose();
});

test("neighbour toggle drains every page, returns all selections and retracts", async () => {
  const root = { ...node("root"), parent_count: "0", child_count: "2" };
  const left = node("left"),
    right = node("right");
  const calls: Record<string, string>[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        calls.push(q);
        return Promise.resolve(
          json(
            q.depth === "0"
              ? page([root])
              : q.cursor
                ? page([right], [edge("root", "right")])
                : page([root, left], [edge("root", "left")], {
                    next_cursor: "next",
                    total_nodes: 3,
                    remaining_nodes: 1,
                  }),
            context(q.branch),
          ),
        );
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  model.connect();
  await model.explore({
    start: "data/root",
    direction: "downstream",
    depth: "0",
  });
  expect(model.expanded(root, "downstream")).toBe(false);
  expect(await model.toggle(root, "downstream")).toEqual(
    new Set(["left", "right"]),
  );
  expect(model.expanded(root, "downstream")).toBe(true);
  expect(model.snapshot().nodes).toHaveLength(3);
  expect(calls.map((c) => c.cursor)).toEqual([undefined, undefined, "next"]);
  expect(await model.toggle(root, "downstream")).toEqual(new Set(["root"]));
  expect(model.snapshot().nodes.map((n) => n.identity)).toEqual(["root"]);
  expect(model.snapshot().edges).toHaveLength(0);
  expect(model.expanded(root, "downstream")).toBe(false);
  workspace.dispose();
});
