import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { ViewHistory, openingViewport, type Visual } from "./view";
import { exportJson, exportSvg } from "./exports";
import { decode, Client } from "../api/client";
import { Workspace } from "../workspace";
import { fixtureTransport, context, json } from "../test-fixtures";
import { ViewEditor } from "./ViewEditor";
import { GraphExplorer } from "./Graph";
import fixtures from "../../../schemas/fixtures/conformance.json";
import type { ApiLineageNodeV1, GraphViewV1 } from "../generated/contracts";
const doc = (): GraphViewV1 =>
  decode(
    "GraphViewV1",
    fixtures.find((f) => f.name === "graph-view-v1")?.value,
  );
const node: ApiLineageNodeV1 = {
  identity: doc().datasets[0]?.identity ?? "",
  paths: ["raw/items"],
  depth: "0",
  external: false,
  producer: true,
  resource_type: "polars_transform",
  publication: "published",
  parent_count: "0",
  child_count: "0",
};
const visual = (): Visual => ({
  nodes: [node],
  edges: [],
  positions: { [node.identity]: { x: 40, y: 80 } },
  selected: [],
  viewport: { x: 5, y: 6, zoom: 1.2 },
  colour: "resource",
});
test("visual undo restores membership, positions and camera without operational handles", () => {
  const h = new ViewHistory(),
    first = visual();
  h.reset(first);
  const second = {
    ...first,
    selected: [node.identity],
    positions: { [node.identity]: { x: 100, y: 20 } },
    viewport: { x: 70, y: 10, zoom: 0.8 },
  };
  h.observe(second);
  h.observe({ ...second, nodes: [], selected: [] });
  expect(h.undo()).toEqual(second);
  expect(h.undo()).toEqual(first);
  expect(h.redo()).toEqual(second);
  h.observe({ ...second, colour: "publication" });
  expect(h.canRedo).toBe(false);
});
test("exports honour explicit labels and metadata, and escape SVG markup", () => {
  const v = {
    ...doc(),
    name: "Private title",
    description: "<script>not executed</script>",
  };
  const hidden = { labels: false, metadata: false };
  for (const output of [
    exportJson(v, visual(), hidden),
    exportSvg(v, visual(), hidden),
  ]) {
    expect(output).not.toContain("Private");
    expect(output).not.toContain("raw/items");
    expect(output).not.toContain("not executed");
    expect(output).not.toContain("<image");
    expect(output).not.toContain("annotations");
    expect(output).not.toContain("groups");
  }
  const svg = exportSvg(v, visual(), { labels: true, metadata: true });
  expect(svg).toContain("&lt;script&gt;");
  expect(svg).not.toContain("<script>");
  expect(svg).toContain("Private title");
});
test("saved view controls preserve edits on conflict and only issue a view mutation", async () => {
  const mutations: string[] = [];
  let saved: GraphViewV1 | null = null;
  const transport = fixtureTransport((path, q) => {
    if (path === "/api/v1/lineage")
      return Promise.resolve(
        json(
          {
            nodes: [node],
            edges: [],
            next_cursor: null,
            total_nodes: 1,
            total_edges: 0,
            remaining_nodes: 0,
            remaining_edges: 0,
            omitted_nodes: 0,
            omitted_edges: 0,
            scope_complete: true,
            external_expanded: false,
          },
          context(q.branch),
        ),
      );
    if (path === "/api/v1/views")
      return Promise.resolve(
        json({
          views: saved
            ? [
                {
                  id: saved.id,
                  revision: saved.revision,
                  name: saved.name,
                  saved_at_us: "1700000000000000",
                  branch: "review",
                },
              ]
            : [],
          next_cursor: null,
        }),
      );
    if (path.startsWith("/api/v1/views/")) return Promise.resolve(json(saved));
    return;
  });
  let conflict = false;
  const client = new Client(async (input, init) => {
    const url = String(input);
    if (url.startsWith("/api/v1/views?")) {
      mutations.push(url);
      if (conflict) return new Response("{}", { status: 409 });
      saved = decode("GraphViewV1", JSON.parse(String(init?.body)));
      saved = { ...saved, revision: saved.revision + 1 };
      return json(saved, context());
    }
    return transport(input, init);
  });
  const workspace = new Workspace(client);
  await workspace.connect("launch");
  const view = render(
    <GraphExplorer
      workspace={workspace}
      focusRequest={{ path: "raw/items", revision: 1 }}
    />,
  );
  await waitFor(() =>
    expect(
      view.container.querySelector('[aria-label="Details for raw/items"]'),
    ).not.toBeNull(),
  );
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(screen.queryByLabelText("Description")).toBeNull();
  fireEvent.change(screen.getByLabelText("Lineage name"), {
    target: { value: "Analysis" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save lineage" }));
  await screen.findByText("Analysis");
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(true);
  expect(saved).not.toHaveProperty("viewport");
  expect(saved).not.toHaveProperty("groups");
  expect(saved).not.toHaveProperty("annotations");
  expect(saved).not.toHaveProperty("filters");
  expect(saved).toHaveProperty("selector", {
    branch: "master",
    fallback: ["public"],
  });
  fireEvent.click(
    screen.getByRole("button", { name: "Edit lineage description" }),
  );
  fireEvent.change(screen.getByLabelText("Lineage description"), {
    target: { value: "My analysis" },
  });
  fireEvent.keyDown(screen.getByLabelText("Lineage description"), {
    key: "Enter",
  });
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(false);
  conflict = true;
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await screen.findByRole("alert");
  expect(screen.getByText("My analysis")).toBeTruthy();
  expect(mutations).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "Lineage actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Open lineage" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "Open Analysis, version 1" }),
  );
  await screen.findByRole("dialog", { name: "Discard unsaved changes?" });
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect(screen.getByText("My analysis")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Close lineage panel" }));
  conflict = false;
  const originalId = (saved as GraphViewV1 | null)?.id;
  fireEvent.click(screen.getByRole("button", { name: "Lineage actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Save as" }));
  expect(screen.queryByLabelText("Description")).toBeNull();
  fireEvent.change(screen.getByLabelText("Lineage name"), {
    target: { value: "Copy" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save lineage" }));
  await screen.findByText("Copy");
  expect((saved as GraphViewV1 | null)?.id).not.toBe(originalId);
  expect(saved).toHaveProperty("revision", 1);
  expect(saved).toHaveProperty("description", "My analysis");
  fireEvent.click(screen.getByRole("button", { name: "Node colouring" }));
  fireEvent.keyDown(screen.getByRole("menu", { name: "Node colouring" }), {
    key: "ArrowDown",
  });
  expect(document.activeElement?.textContent).toContain("Publication");
  fireEvent.click(screen.getByRole("menuitemradio", { name: "Publication" }));
  expect(
    screen.getByRole("button", { name: "Node colouring" }).textContent,
  ).toContain("Publication");
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(false);
  view.unmount();
  workspace.dispose();
});

test("a shared view reopens membership after a superseded prefetch abort", async () => {
  const document = {
    ...doc(),
    selector: { branch: "master", fallback: ["public"] },
  };
  window.history.replaceState(null, "", `/?view=${document.id}`);
  let interrupted = 0;
  const transport = fixtureTransport((path, query) => {
    if (path.startsWith("/api/v1/views/"))
      return Promise.resolve(json(document));
    if (path === "/api/v1/lineage") {
      if (interrupted < 2) {
        interrupted++;
        return workspace.refresh().then(() => {
          throw new DOMException("Superseded", "AbortError");
        });
      }
      return Promise.resolve(
        json(
          {
            nodes: [node],
            edges: [],
            next_cursor: null,
            total_nodes: 1,
            total_edges: 0,
            remaining_nodes: 0,
            remaining_edges: 0,
            omitted_nodes: 0,
            omitted_edges: 0,
            scope_complete: true,
            external_expanded: false,
          },
          context(query.branch),
        ),
      );
    }
    return undefined;
  });
  const workspace = new Workspace(new Client(transport));
  await workspace.connect("launch");
  const alert = vi.spyOn(window, "alert").mockImplementation(() => {});
  const view = render(<GraphExplorer workspace={workspace} />);
  await waitFor(() =>
    expect(
      view.container.querySelector('[aria-label="Details for raw/items"]'),
    ).not.toBeNull(),
  );
  expect(interrupted).toBe(2);
  expect(alert).not.toHaveBeenCalled();
  view.unmount();
  workspace.dispose();
  window.history.replaceState(null, "", "/");
  alert.mockRestore();
});

test("moving a saved node enables Save; restoring its position returns to clean state", async () => {
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const second = {
    ...node,
    identity:
      "dataset:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb:cccccccc-cccc-4ccc-8ccc-cccccccccccc",
  };
  const v = {
    ...visual(),
    nodes: [node, second],
    positions: { ...visual().positions, [second.identity]: { x: 300, y: 80 } },
  };
  const saved = {
    ...doc(),
    datasets: [...v.nodes].reverse().map((n) => ({
      identity: n.identity,
      position: v.positions[n.identity] ?? { x: 0, y: 0 },
    })),
    selector: { branch: "master", fallback: ["public"] },
  };
  const props = {
    workspace,
    opened: saved,
    onOpen: async () => {},
    onNew: () => {},
    onMessage: () => {},
    titleHost: null,
    actionsHost: null,
  };
  const view = render(<ViewEditor {...props} visual={v} />);
  view.rerender(
    <ViewEditor
      {...props}
      visual={{ ...v, viewport: { x: 900, y: -700, zoom: 0.3 } }}
    />,
  );
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(true);
  view.rerender(
    <ViewEditor
      {...props}
      visual={{
        ...v,
        positions: { ...v.positions, [node.identity]: { x: 500, y: 80 } },
      }}
    />,
  );
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(false);
  view.rerender(<ViewEditor {...props} visual={v} />);
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Lineage actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Save as" }));
  fireEvent.change(screen.getByLabelText("Lineage name"), {
    target: { value: "Cancelled copy" },
  });
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(true);
  view.unmount();
  workspace.dispose();
});

test("opening camera uses the node bounds at fixed zoom, independently of prior pan/zoom", () => {
  expect(openingViewport([], 1000, 600)).toEqual({ x: 0, y: 0, zoom: 1 });
  expect(
    openingViewport(
      [
        { identity: "a", position: { x: -200, y: 80 } },
        { identity: "b", position: { x: 400, y: 180 } },
      ],
      1000,
      600,
    ),
  ).toEqual({ x: 275, y: 150, zoom: 1 });
});

test("lineage picker searches all pages, formats versions/timestamps and fences stale replies", async () => {
  let resolveOld!: (response: Response) => void;
  const requested: Record<string, string>[] = [];
  const summary = {
    id: doc().id,
    name: "Review workspace",
    revision: 4,
    saved_at_us: "1700000000000000",
    branch: "review",
  };
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (path === "/api/v1/views") {
          requested.push(q);
          if (q.search === "old")
            return new Promise((r) => {
              resolveOld = r;
            });
          if (q.search === "fail")
            return Promise.resolve(new Response("{}", { status: 500 }));
          return Promise.resolve(
            json({
              views: q.search === "absent" ? [] : [summary],
              next_cursor: q.after ? null : summary.id,
            }),
          );
        }
        return;
      }),
    ),
  );
  await workspace.connect("launch");
  const rendered = render(
    <ViewEditor
      workspace={workspace}
      visual={visual()}
      opened={null}
      onOpen={async () => {}}
      onNew={() => {}}
      onMessage={() => {}}
      titleHost={null}
      actionsHost={null}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Lineage actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Open lineage" }));
  await screen.findByRole("button", {
    name: "Open Review workspace, version 4",
  });
  expect(screen.getByText(/Last saved/).textContent).toMatch(
    /Last saved \d{4}-\d{2}-\d{2} \d{2}:\d{2}$/u,
  );
  expect(screen.getByText("v4")).toBeTruthy();
  expect(screen.getByTitle("Branch review").textContent).toContain("review");
  fireEvent.click(screen.getByRole("button", { name: "Next page" }));
  await waitFor(() => expect(requested.at(-1)?.after).toBe(summary.id));
  const input = screen.getByRole("searchbox", { name: "Search lineages" });
  fireEvent.change(input, { target: { value: "RVW" } });
  await waitFor(() => expect(requested.at(-1)).toEqual({ search: "RVW" }));
  await waitFor(() =>
    expect(rendered.container.querySelectorAll("mark")).toHaveLength(3),
  );
  fireEvent.change(input, { target: { value: "old" } });
  await waitFor(() => expect(resolveOld).toBeDefined());
  fireEvent.change(input, { target: { value: "absent" } });
  await screen.findByText("No lineages match this search.");
  resolveOld(json({ views: [summary], next_cursor: null }));
  await waitFor(() =>
    expect(screen.queryByText("Review workspace")).toBeNull(),
  );
  fireEvent.change(input, { target: { value: "fail" } });
  await screen.findByRole("alert");
  expect(screen.queryByText("Review workspace")).toBeNull();
  rendered.unmount();
  workspace.dispose();
});

test("first open keeps the picker mounted and avoids reloading equivalent implicit context; New resets saved state", async () => {
  const saved = {
    ...doc(),
    revision: 2,
    selector: { branch: "master", fallback: ["public"] },
  };
  let resolveMembership!: (response: Response) => void;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) => {
        if (path === "/api/v1/views")
          return Promise.resolve(
            json({
              views: [
                {
                  id: saved.id,
                  name: saved.name,
                  revision: 2,
                  branch: "master",
                  saved_at_us: "1700000000000000",
                },
              ],
              next_cursor: null,
            }),
          );
        if (path.startsWith("/api/v1/views/"))
          return Promise.resolve(json(saved));
        if (path === "/api/v1/lineage")
          return new Promise((resolve) => {
            resolveMembership = resolve;
          });
        return;
      }),
    ),
  );
  await workspace.connect("launch");
  const states: string[] = [];
  const unsubscribe = workspace.subscribe(() =>
    states.push(workspace.snapshot().kind),
  );
  const view = render(<GraphExplorer workspace={workspace} />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Lineage actions" }),
  );
  fireEvent.click(screen.getByRole("menuitem", { name: "Open lineage" }));
  const picker = await screen.findByRole("dialog", { name: "Open lineage" });
  fireEvent.click(
    await screen.findByRole("button", {
      name: "Open Synthetic view, version 2",
    }),
  );
  await waitFor(() => expect(resolveMembership).toBeDefined());
  expect(screen.getByRole("dialog", { name: "Open lineage" })).toBe(picker);
  expect(states).not.toContain("loading");
  await act(async () =>
    resolveMembership(
      json(
        {
          nodes: [node],
          edges: [],
          next_cursor: null,
          total_nodes: 1,
          total_edges: 0,
          remaining_nodes: 0,
          remaining_edges: 0,
          omitted_nodes: 0,
          omitted_edges: 0,
          scope_complete: true,
          external_expanded: false,
        },
        context(),
      ),
    ),
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await screen.findByText("Synthetic view");
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Lineage actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "New" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(view.container.querySelectorAll(".react-flow__node")).toHaveLength(0);
  expect(screen.queryByText("Synthetic view")).toBeNull();
  expect(
    screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
  ).toBe(true);
  expect(
    screen
      .getByRole("button", { name: "Undo view change" })
      .hasAttribute("disabled"),
  ).toBe(true);
  expect(states).not.toContain("loading");
  unsubscribe();
  view.unmount();
  workspace.dispose();
});

test.each([false, true])(
  "New protects dirty %s saved state with a custom cancellable dialog",
  async (persisted) => {
    const workspace = new Workspace(new Client(fixtureTransport()));
    await workspace.connect("launch");
    const onNew = vi.fn();
    const props = {
      workspace,
      visual: visual(),
      opened: persisted ? doc() : null,
      onNew,
      onOpen: async () => {},
      onMessage: () => {},
      titleHost: null,
      actionsHost: null,
    };
    const view = render(<ViewEditor {...props} />);
    const chooseNew = () => {
      fireEvent.click(screen.getByRole("button", { name: "Lineage actions" }));
      fireEvent.click(screen.getByRole("menuitem", { name: "New" }));
    };
    chooseNew();
    expect(
      screen.getByRole("dialog", { name: "Discard unsaved changes?" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(onNew).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"),
    ).toBe(false);
    chooseNew();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onNew).not.toHaveBeenCalled();
    chooseNew();
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(onNew).toHaveBeenCalledOnce();
    view.unmount();
    workspace.dispose();
  },
);
