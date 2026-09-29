import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
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
import { GraphCache } from "./cache";
const node = (id: string, external = false): ApiLineageNodeV1 => ({
  identity: id,
  paths: [`data/${id}`],
  depth: "0",
  external,
  producer: !external,
  resource_type: "unknown",
  publication: "unknown",
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
        if (q.connections)
          return Promise.resolve(json(page([]), context(q.branch)));
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
  expect(model.snapshot().edges).toHaveLength(2);
  expect(model.snapshot().nodes.some((n) => n.identity === "a")).toBe(false);
  expect(model.snapshot().page).toBeNull();
  stop();
  workspace.dispose();
});
test("500-node guard pauses presentation without silently discarding continuation", async () => {
  let batch = 0;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        if (q.connections)
          return Promise.resolve(json(page([]), context(q.branch)));
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
        if (q.connections)
          return Promise.resolve(json(page([]), context(q.branch)));
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
        if (q.connections)
          return Promise.resolve(json(page([]), context(q.branch)));
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
test("explicit layout foundation fences stale workers and manual movement cancels layout", () => {
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
  expect(positions.snapshot().positions.b).toBeUndefined();
  workers[1]?.onerror?.(new ErrorEvent("error"));
  expect(positions.snapshot().error).toContain("worker failed");
  expect(positions.snapshot().positions.a).toEqual({ x: 900, y: 700 });
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
        if (q.connections)
          return Promise.resolve(json(page([]), context(q.branch)));
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

test("adding a shared parent reveals connections to every visible child and fails atomically", async () => {
  let fail = false;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        if (q.connections)
          return fail
            ? Promise.reject(new Error("Connection lookup failed"))
            : Promise.resolve(
                json(
                  page([], [edge("root", "left"), edge("root", "right")]),
                  context(q.branch),
                ),
              );
        const id = q.start?.replace("data/", "") ?? "";
        return Promise.resolve(json(page([node(id)]), context(q.branch)));
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  model.connect();
  for (const id of ["left", "right", "root"])
    await model.explore({
      start: `data/${id}`,
      direction: "upstream",
      depth: "0",
    });
  expect(
    model
      .snapshot()
      .edges.map((e) => e.consumer)
      .sort(),
  ).toEqual(["left", "right"]);
  model.remove(new Set(["root"]));
  fail = true;
  await model.explore({
    start: "data/root",
    direction: "upstream",
    depth: "0",
  });
  expect(model.snapshot().nodes.map((n) => n.identity)).toEqual([
    "left",
    "right",
  ]);
  expect(model.snapshot().edges).toHaveLength(2); // Hidden endpoints stay available for re-add.
  expect(model.snapshot().message).toContain("coordinator is disconnected");
  workspace.dispose();
});

test("adding, removing and reordering nodes preserves manual positions without starting layout", () => {
  const create = vi.fn(fakeWorker);
  const positions = new Positions(create);
  positions.sync([node("a"), node("b")]);
  const initial = positions.snapshot().positions;
  positions.move("a", { x: 700, y: 400 });
  positions.sync([node("c"), node("b"), node("a")]);
  expect(positions.snapshot().positions.a).toEqual({ x: 700, y: 400 });
  expect(positions.snapshot().positions.b).toEqual(initial.b);
  const added = positions.snapshot().positions.c;
  positions.sync([node("c"), node("a")]);
  expect(positions.snapshot().positions.c).toEqual(added);
  expect(positions.snapshot().positions.a).toEqual({ x: 700, y: 400 });
  expect(positions.snapshot().positions).not.toHaveProperty("b");
  expect(create).not.toHaveBeenCalled();
});
test("multi-root expansion uses selected depth for every root and reconciles cross edges", async () => {
  const queries: Record<string, string>[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        queries.push(q);
        return Promise.resolve(
          json(
            page(
              [node(q.start === "data/a" ? "a" : "b")],
              q.start === "data/b" ? [edge("a", "b")] : [],
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
  await model.expand([node("a"), node("b")], "downstream", "3");
  expect(
    queries.filter((q) => q.start).map((q) => [q.start, q.depth, q.direction]),
  ).toEqual([
    ["data/a", "3", "downstream"],
    ["data/b", "3", "downstream"],
  ]);
  expect(model.snapshot().nodes).toHaveLength(2);
  expect(model.snapshot().edges).toEqual([edge("a", "b")]);
  stop();
  workspace.dispose();
});

test("expansion places new ancestors left and descendants right without moving existing nodes", () => {
  for (const direction of ["upstream", "downstream"] as const) {
    const positions = new Positions(vi.fn(fakeWorker));
    positions.move("root", { x: 710, y: 310 });
    const edges =
      direction === "upstream"
        ? [edge("near", "root"), edge("far", "near"), edge("sibling", "root")]
        : [edge("root", "near"), edge("near", "far"), edge("root", "sibling")];
    positions.sync(
      [node("far"), node("sibling"), node("root"), node("near")],
      edges,
      { roots: ["root"], direction },
    );
    const p = positions.snapshot().positions;
    expect(p.root).toEqual({ x: 710, y: 310 });
    const sign = direction === "upstream" ? -1 : 1;
    expect(sign * ((p.near?.x ?? 0) - 710)).toBeGreaterThan(270);
    expect(sign * ((p.sibling?.x ?? 0) - 710)).toBeGreaterThan(270);
    expect(sign * ((p.far?.x ?? 0) - (p.near?.x ?? 0))).toBeGreaterThan(270);
    expect(p.sibling).not.toEqual(p.near);
  }
});

test("cached additions and complete neighbour expansions commit immediately with every cross edge", async () => {
  const root = { ...node("root"), parent_count: "0", child_count: "2" };
  let reads = 0;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) => {
        if (path.endsWith("/lineage")) {
          reads++;
          return Promise.resolve(
            json(
              page(
                [root, node("left"), node("right")],
                [edge("root", "left"), edge("root", "right")],
              ),
              context(),
            ),
          );
        }
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const stop = model.connect();
  await model.add("data/left");
  model.remove(new Set(["root", "right"]));
  const added = model.add("data/root");
  expect(model.snapshot().nodes.map((n) => n.identity)).toEqual([
    "left",
    "root",
  ]);
  expect(reads).toBe(1);
  await added;
  const expanded = model.expand([root], "downstream", "1");
  expect(model.snapshot().nodes).toHaveLength(3);
  expect(model.snapshot().edges).toHaveLength(2);
  expect(reads).toBe(1);
  expect(await expanded).toEqual(new Set(["left", "right"]));
  stop();
  workspace.dispose();
});

test("cache never treats an incomplete child set as a complete expansion and resets late reads", async () => {
  let resolve!: (response: Response) => void;
  const root = { ...node("root"), parent_count: "0", child_count: "2" };
  let first = true;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) => {
        if (!path.endsWith("/lineage")) return;
        if (first) {
          first = false;
          return Promise.resolve(
            json(page([root, node("left")], [edge("root", "left")]), context()),
          );
        }
        return new Promise((r) => {
          resolve = r;
        });
      }),
    ),
  );
  await workspace.connect("launch");
  const cache = new GraphCache(workspace);
  await cache.lookup(["data/root", "data/left"]);
  expect(cache.neighbourhood([root], "downstream", "1")).toBeNull();
  expect(
    cache.neighbourhood([node("left")], "upstream", "1")?.nodes,
  ).toHaveLength(2);
  const pending = cache.lookup(["data/right"]);
  cache.reset();
  resolve(json(page([node("right")]), context()));
  await expect(pending).rejects.toThrow();
  expect(cache.known(["data/root"])).toBeNull();
  expect(cache.known(["data/right"])).toBeNull();
  workspace.dispose();
});

test("catalogue prefetch is bounded and concurrent clicks reuse its request", async () => {
  let resolve!: (response: Response) => void;
  const queries: Record<string, string>[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        queries.push(q);
        return new Promise((r) => {
          resolve = r;
        });
      }),
    ),
  );
  await workspace.connect("launch");
  const cache = new GraphCache(workspace);
  const warm = cache.warm(Array.from({ length: 105 }, (_, i) => `data/${i}`));
  const click = cache.lookup(["data/2"]);
  expect(queries).toHaveLength(1);
  expect(JSON.parse(queries[0]?.lookup ?? "[]")).toHaveLength(100);
  resolve(
    json(
      page(Array.from({ length: 100 }, (_, i) => node(String(i)))),
      context(),
    ),
  );
  await warm;
  expect((await click).nodes.map((n) => n.paths[0])).toEqual(["data/2"]);
  expect(cache.known(["data/104"])).toBeNull();
  workspace.dispose();
});

test.each(["before", "after"] as const)(
  "lineage selection survives catalogue focus frame %s the gesture and preserves keyboard shortcuts",
  async (focusTiming) => {
    vi.useFakeTimers({
      toFake: ["requestAnimationFrame", "cancelAnimationFrame"],
    });
    try {
      const workspace = new Workspace(
        new Client(
          fixtureTransport((path) =>
            path.endsWith("/lineage")
              ? Promise.resolve(json(page([node("a"), node("b")]), context()))
              : undefined,
          ),
        ),
      );
      await workspace.connect("launch");
      const view = render(
        <>
          <input aria-label="Search outside graph" />
          <GraphExplorer
            workspace={workspace}
            focusRequest={{ path: "data/a", revision: 1 }}
          />
        </>,
      );
      await waitFor(() =>
        expect(
          view.container.querySelectorAll(".react-flow__node"),
        ).toHaveLength(2),
      );
      if (focusTiming === "before") {
        await act(async () => {
          vi.advanceTimersToNextFrame();
        });
        expect(
          screen.getByRole("button", { name: "1 node selected" }),
        ).toBeTruthy();
        const selectionMenu = view.container.querySelector(
          ".graph-options > summary",
        );
        if (!selectionMenu) throw new Error("Missing selection menu");
        fireEvent.click(selectionMenu);
        fireEvent.click(screen.getByRole("button", { name: "Clear all" }));
        expect(
          screen.getByRole("button", { name: "0 nodes selected" }),
        ).toBeTruthy();
      }
      const diagram = screen.getByLabelText("Dataset lineage diagram");
      diagram.focus();
      const pane = view.container.querySelector(".react-flow__pane");
      if (!pane) throw new Error("Missing graph pane");
      fireEvent.keyDown(window, {
        key: "Shift",
        code: "ShiftLeft",
        shiftKey: true,
      });
      const pointer = (type: string, x: number, y: number) => {
        const event = new MouseEvent(type, {
          bubbles: true,
          button: 0,
          clientX: x,
          clientY: y,
          shiftKey: true,
        });
        Object.defineProperties(event, {
          isPrimary: { value: true },
          pointerId: { value: 1 },
        });
        fireEvent(pane, event);
      };
      pointer("pointerdown", 1, 1);
      pointer("pointermove", 700, 500);
      expect(
        view.container.querySelector(".react-flow__selection"),
      ).not.toBeNull();
      expect(
        screen.getByRole("button", { name: "2 nodes selected" }),
      ).toBeTruthy();
      pointer("pointerup", 700, 500);
      await act(async () => {
        if (focusTiming === "after") vi.advanceTimersToNextFrame();
      });
      fireEvent.keyUp(window, { key: "Shift", code: "ShiftLeft" });
      expect(view.container.querySelector(".react-flow__selection")).toBeNull();
      expect(
        view.container.querySelector(".react-flow__nodesselection"),
      ).toBeNull();
      expect(
        screen.getByRole("button", { name: "2 nodes selected" }),
      ).toBeTruthy();
      fireEvent.keyDown(diagram, { key: "a", metaKey: true });
      expect(
        screen.getByRole("button", { name: "2 nodes selected" }),
      ).toBeTruthy();
      fireEvent.keyDown(diagram, { key: "Backspace" });
      expect(
        screen.getByRole("button", { name: "0 nodes selected" }),
      ).toBeTruthy();
      const search = screen.getByRole("textbox", {
        name: "Search outside graph",
      });
      search.focus();
      expect(fireEvent.keyDown(search, { key: "a", metaKey: true })).toBe(true);
      expect(
        screen.getByRole("button", { name: "Node colouring" }).textContent,
      ).toContain("Resource Type");
      expect(
        view.container.querySelector(".react-flow__attribution"),
      ).toBeNull();
      expect(screen.queryByText(/Open the catalogue/)).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Toggle legend" }));
      expect(screen.getByLabelText("Graph legend").textContent).toContain(
        "Polars Transform",
      );
      workspace.dispose();
    } finally {
      vi.useRealTimers();
    }
  },
);

test("rapid uncached additions both appear, while removal cancels unfinished additions", async () => {
  const pending = new Map<string, (response: Response) => void>();
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (!path.endsWith("/lineage")) return;
        return new Promise((resolve) => {
          pending.set(q.lookup ?? "", resolve);
        });
      }),
    ),
  );
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const stop = model.connect();
  const first = model.add("data/a"),
    second = model.add("data/b");
  pending.get('["data/b"]')?.(json(page([node("b")]), context()));
  await second;
  expect(model.snapshot().busy).toBe(true);
  pending.get('["data/a"]')?.(json(page([node("a")]), context()));
  await first;
  expect(
    model
      .snapshot()
      .nodes.map((n) => n.identity)
      .sort(),
  ).toEqual(["a", "b"]);
  expect(model.snapshot().busy).toBe(false);
  const late = model.add("data/c");
  model.remove(new Set(["a"]));
  pending.get('["data/c"]')?.(json(page([node("c")]), context()));
  await late;
  expect(model.snapshot().nodes.map((n) => n.identity)).toEqual(["b"]);
  stop();
  workspace.dispose();
});

test("legend rows select exactly matching visible nodes in each mode", async () => {
  const local = {
    ...node("a"),
    resource_type: "polars_transform" as const,
    publication: "published" as const,
  };
  const foreign = {
    ...node("foreign", true),
    resource_type: "external" as const,
  };
  const missing = { ...node("missing"), publication: "missing" as const };
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) =>
        path.endsWith("/lineage")
          ? Promise.resolve(
              json(
                page(
                  [local, foreign, missing],
                  [
                    edge("a", "missing"),
                    edge("foreign", "missing", "check", "validation"),
                  ],
                ),
                context(),
              ),
            )
          : undefined,
      ),
    ),
  );
  await workspace.connect("launch");
  const view = render(
    <GraphExplorer
      workspace={workspace}
      focusRequest={{ path: "data/a", revision: 1 }}
    />,
  );
  await waitFor(() =>
    expect(view.container.querySelectorAll(".dataset-node")).toHaveLength(3),
  );
  fireEvent.click(screen.getByRole("button", { name: "Toggle legend" }));
  fireEvent.click(
    screen.getByRole("button", { name: "Select External Dataset nodes" }),
  );
  await waitFor(() =>
    expect(
      view.container
        .querySelector(".react-flow__node.selected")
        ?.getAttribute("data-id"),
    ).toBe("foreign"),
  );
  expect(screen.queryByText(/Counts show visible datasets/)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Node colouring" }));
  fireEvent.click(screen.getByRole("menuitemradio", { name: "Publication" }));
  fireEvent.click(
    screen.getByRole("button", {
      name: "Select Not built on these branches nodes",
    }),
  );
  await waitFor(() =>
    expect(
      view.container
        .querySelector(".react-flow__node.selected")
        ?.getAttribute("data-id"),
    ).toBe("missing"),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Select Published nodes" }),
  );
  await waitFor(() =>
    expect(
      view.container
        .querySelector(".react-flow__node.selected")
        ?.getAttribute("data-id"),
    ).toBe("a"),
  );
  view.unmount();
  workspace.dispose();
});
