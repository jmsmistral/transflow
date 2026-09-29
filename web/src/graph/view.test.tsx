import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { ViewHistory, groupsFor, rollup, type Visual } from "./view";
import { NoteText } from "./Note";
import { exportJson, exportSvg } from "./exports";
import { decode, Client } from "../api/client";
import { Workspace } from "../workspace";
import { fixtureTransport, context, json } from "../test-fixtures";
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
  groups: [],
  annotations: doc().annotations,
  viewport: { x: 5, y: 6, zoom: 1.2 },
  colour: "resource",
});
test("visual undo restores hidden membership, positions, annotations, grouping and camera without operational handles", () => {
  const h = new ViewHistory(),
    first = visual();
  h.reset(first);
  const groups = groupsFor(first.nodes, first.positions, () => "Inputs");
  const second = {
    ...first,
    groups,
    selected: [node.identity],
    positions: { [node.identity]: { x: 100, y: 20 } },
    viewport: { x: 70, y: 10, zoom: 0.8 },
  };
  h.observe(second);
  h.observe({
    ...second,
    nodes: [],
    annotations: [],
    selected: [],
    groups: [],
  });
  expect(h.undo()).toEqual(second);
  expect(h.undo()).toEqual(first);
  expect(h.redo()).toEqual(second);
  h.observe({ ...second, colour: "publication" });
  expect(h.canRedo).toBe(false);
  const group = groups[0];
  if (!group) throw new Error("Expected group");
  expect(rollup(group, first.nodes)).toContain(
    "1 published · 0 not built · 0 unknown",
  );
});
test("restricted markdown renders markup and URLs as text, with no executable or remote elements", () => {
  const view = render(
    <NoteText
      markdown
      text={
        "**Bold** *emphasis* `code` <script>alert(1)</script> ![x](https://example.invalid/x) [bad](javascript:alert(1))"
      }
    />,
  );
  expect(
    view.container.querySelectorAll("script,img,iframe,a,svg"),
  ).toHaveLength(0);
  expect(view.container.querySelector("strong")?.textContent).toBe("Bold");
  expect(view.container.textContent).toContain("<script>");
});
test("exports honour explicit label, metadata and note inclusion and escape SVG markup", () => {
  const annotation = doc().annotations[0];
  if (!annotation) throw new Error("Expected note");
  const v = {
    ...doc(),
    name: "Private title",
    description: "Private description",
    annotations: [
      {
        ...annotation,
        text: "<script>not executed</script> & remote",
      },
    ],
  };
  const hidden = { labels: false, metadata: false, notes: false };
  for (const output of [
    exportJson(v, visual(), hidden),
    exportSvg(v, visual(), hidden),
  ]) {
    expect(output).not.toContain("Private");
    expect(output).not.toContain("raw/items");
    expect(output).not.toContain("not executed");
    expect(output).not.toContain("<image");
  }
  const svg = exportSvg(v, visual(), {
    labels: true,
    metadata: true,
    notes: true,
  });
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
            ? [{ id: saved.id, revision: saved.revision, name: saved.name }]
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
  fireEvent.click(screen.getByRole("button", { name: /^Views/u }));
  fireEvent.change(screen.getByLabelText("View name"), {
    target: { value: "Analysis" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save view" }));
  await screen.findByText("Analysis · Saved");
  fireEvent.click(screen.getByRole("button", { name: "Notes" }));
  fireEvent.change(screen.getByLabelText("Annotation"), {
    target: { value: "**Analysis**" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add note" }));
  await waitFor(() =>
    expect(view.container.querySelector(".view-note")).not.toBeNull(),
  );
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  conflict = true;
  fireEvent.click(screen.getByRole("button", { name: "Save view" }));
  await screen.findByRole("alert");
  expect((screen.getByLabelText("View name") as HTMLInputElement).value).toBe(
    "Analysis",
  );
  expect(view.container.querySelector(".view-note")).not.toBeNull();
  expect(mutations).toHaveLength(2);
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
  fireEvent.click(screen.getByRole("button", { name: "Open" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "Analysis · revision 1" }),
  );
  await waitFor(() => expect(confirm).toHaveBeenCalled());
  expect(view.container.querySelector(".view-note")).not.toBeNull();
  confirm.mockRestore();
  view.unmount();
  workspace.dispose();
});

test("a shared view reopens membership after a superseded prefetch abort", async () => {
  const document = doc();
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
