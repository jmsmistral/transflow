import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Client } from "../api/client";
import { Workspace } from "../workspace";
import {
  context,
  dataset,
  datasetId,
  fixtureTransport,
  json,
  workspaceId,
} from "../test-fixtures";
import { DatasetTooltip } from "./Tooltip";
import { CatalogueSearch, matchPositions } from "./Search";

const node = {
  identity: `dataset:${workspaceId}:${datasetId}`,
  paths: [dataset.path],
  depth: "0",
  producer: false,
  external: false,
  resource_type: "unknown" as const,
  publication: "unknown" as const,
  parent_count: "0",
  child_count: "0",
};
test("tooltip stays absent while loading, then appears complete, and ignores completion after unmount", async () => {
  let resolve!: (response: Response) => void;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) =>
        path.endsWith(`/datasets/${datasetId}`)
          ? new Promise((r) => {
              resolve = r;
            })
          : undefined,
      ),
    ),
  );
  await workspace.connect("launch");
  const view = render(<DatasetTooltip node={node} workspace={workspace} />);
  expect(screen.queryByRole("tooltip")).toBeNull();
  expect(screen.queryByText(/Loading metadata/)).toBeNull();
  await act(async () => {
    resolve(json(dataset, context()));
  });
  expect(screen.getByRole("tooltip").textContent).toContain(dataset.path);
  view.unmount();
  const second = render(<DatasetTooltip node={node} workspace={workspace} />);
  second.unmount();
  await act(async () => {
    resolve(json(dataset, context()));
  });
  expect(screen.queryByRole("tooltip")).toBeNull();
  workspace.dispose();
});
test("catalogue filters automatically, highlights fuzzy matches and identifies visible datasets", async () => {
  const calls: string[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (path.endsWith("/datasets") && q.fuzzy !== undefined) {
          calls.push(q.fuzzy);
          return Promise.resolve(
            json(
              {
                entries: q.fuzzy === "absent" ? [] : [dataset],
                total: q.fuzzy === "absent" ? "0" : "1",
                next_cursor: null,
                schema: null,
                freshness: "unknown",
              },
              context(q.branch),
            ),
          );
        }
      }),
    ),
  );
  await workspace.connect("launch");
  const select = vi.fn();
  render(
    <CatalogueSearch
      workspace={workspace}
      active
      visible={[node.identity]}
      select={select}
    />,
  );
  await screen.findByRole("img", { name: "Already in lineage" });
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: "rwe" } });
  const result = await screen.findByRole("button", { name: /raw\/example/ });
  await waitFor(() => expect(calls).toContain("rwe"));
  expect(result.querySelectorAll("mark")).toHaveLength(3);
  fireEvent.click(result);
  expect(select).toHaveBeenCalledWith(dataset);
  fireEvent.change(screen.getByRole("searchbox"), {
    target: { value: "absent" },
  });
  await screen.findByText("No datasets match this search.");
  expect(matchPositions("raw/example", "RWE")).toEqual(new Set([0, 2, 4]));
  expect(matchPositions("raw/example", "zz")).toEqual(new Set());
  workspace.dispose();
});

test("node pointer drag dismisses the tooltip until a fresh hover, including pending metadata", async () => {
  let resolve!: (response: Response) => void;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) => {
        if (path.endsWith(`/datasets/${datasetId}`))
          return new Promise((r) => {
            resolve = r;
          });
        if (path.endsWith("/lineage"))
          return Promise.resolve(
            json(
              {
                nodes: [node],
                edges: [],
                total_nodes: 1,
                total_edges: 0,
                next_cursor: null,
                remaining_nodes: 0,
                remaining_edges: 0,
                omitted_nodes: 0,
                omitted_edges: 0,
                scope_complete: true,
                external_expanded: false,
              },
              context(),
            ),
          );
        return;
      }),
    ),
  );
  await workspace.connect("launch");
  const { GraphExplorer } = await import("./Graph");
  const view = render(
    <GraphExplorer
      workspace={workspace}
      focusRequest={{ path: dataset.path, revision: 1 }}
    />,
  );
  const label = await screen.findByLabelText(`Details for ${dataset.path}`);
  fireEvent.mouseEnter(label);
  await waitFor(() => expect(resolve).toBeDefined());
  await act(async () => resolve(json(dataset, context())));
  expect(screen.getByRole("tooltip")).toBeTruthy();
  fireEvent.pointerDown(label);
  fireEvent.focus(label);
  fireEvent.mouseEnter(label, { buttons: 1 });
  expect(screen.queryByRole("tooltip")).toBeNull();
  fireEvent.mouseLeave(label);
  fireEvent.mouseEnter(label, { buttons: 0 });
  await act(async () => resolve(json(dataset, context())));
  expect(screen.getByRole("tooltip")).toBeTruthy();
  const wrapper = label.closest(".dataset-node");
  if (!wrapper) throw new Error("Missing node");
  fireEvent.pointerDown(wrapper);
  expect(screen.queryByRole("tooltip")).toBeNull();
  fireEvent.mouseLeave(label);
  fireEvent.mouseEnter(label);
  fireEvent.pointerDown(label);
  await act(async () => resolve(json(dataset, context())));
  expect(screen.queryByRole("tooltip")).toBeNull();
  fireEvent.blur(label);
  fireEvent.focus(label);
  await act(async () => resolve(json(dataset, context())));
  expect(screen.getByRole("tooltip")).toBeTruthy();
  view.unmount();
  workspace.dispose();
});
