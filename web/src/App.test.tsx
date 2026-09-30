import { fireEvent, render, screen } from "@testing-library/react";
import { userEvent } from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { App } from "./App";
import { Button, ErrorBoundary, Status } from "./components";

test("disconnected preview exposes no invented datasets or build controls", () => {
  render(<App />);
  expect(
    screen.getByRole("heading", { name: "Connect to a local coordinator" }),
  ).toBeTruthy();
  expect(screen.getByText("No dataset information loaded")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^build$/i })).toBeNull();
});

test("theme button supports keyboard activation and preserves connection state", async () => {
  const user = userEvent.setup();
  render(<App />);
  const button = screen.getByRole("button", { name: "Dark theme" });
  button.focus();
  await user.keyboard("{Enter}");
  expect(button.getAttribute("aria-pressed")).toBe("true");
  expect(screen.getAllByText("Not connected").length).toBeGreaterThan(0);
  await user.keyboard(" ");
  expect(button.getAttribute("aria-pressed")).toBe("false");
});

test("button defaults to non-submit and disabled actions do not fire", async () => {
  const click = vi.fn();
  const user = userEvent.setup();
  render(
    <Button disabled onClick={click}>
      Unavailable action
    </Button>,
  );
  const button = screen.getByRole("button");
  expect(button.getAttribute("type")).toBe("button");
  await user.click(button);
  expect(click).not.toHaveBeenCalled();
});

test("status values are rendered as text", () => {
  render(<Status>{'<img src=x onerror="alert(1)">'}</Status>);
  expect(screen.getByText(/<img/)).toBeTruthy();
  expect(screen.queryByRole("img")).toBeNull();
});

test("render failure has a safe recovery message without exposing error details", () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  function Broken(): never {
    throw new Error("synthetic private detail");
  }
  render(
    <ErrorBoundary>
      <Broken />
    </ErrorBoundary>,
  );
  expect(
    screen.getByRole("heading", {
      name: "The workspace could not be displayed",
    }),
  ).toBeTruthy();
  expect(screen.queryByText(/synthetic private detail/)).toBeNull();
});

test("button forwards the accessible name and click callback", () => {
  const click = vi.fn();
  render(
    <Button aria-label="Open details" onClick={click}>
      Details
    </Button>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Open details" }));
  expect(click).toHaveBeenCalledOnce();
});

test("keyboard tabs and resize controls preserve panel state across branch changes", async () => {
  const { Workspace } = await import("./workspace");
  const { Client } = await import("./api/client");
  const { fixtureTransport } = await import("./test-fixtures");
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const user = userEvent.setup();
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("button", { name: "Catalogue" }));
  const resize = screen.getByRole("separator", { name: "Resize inspector" });
  resize.focus();
  await user.keyboard("{ArrowLeft}{End}");
  expect(resize.getAttribute("aria-valuenow")).toBe("55");
  expect(screen.queryByRole("tabpanel")).toBeNull();
  const preview = screen.getByRole("tab", { name: "Preview" });
  preview.focus();
  await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}");
  expect(screen.getByRole("tabpanel")).toBeTruthy();
  expect(
    screen.getByRole("tab", { name: "Code" }).getAttribute("aria-selected"),
  ).toBe("true");
  await user.click(screen.getByRole("button", { name: "Branch: master" }));
  await user.type(
    screen.getByRole("textbox", { name: "Search branches" }),
    "feat",
  );
  await user.click(screen.getByRole("button", { name: "feature" }));
  await vi.waitFor(() => expect(workspace.snapshot().kind).toBe("ready"));
  expect(
    screen
      .getByRole("separator", { name: "Resize inspector" })
      .getAttribute("aria-valuenow"),
  ).toBe("55");
  expect(
    screen.getByRole("tab", { name: "Code" }).getAttribute("aria-selected"),
  ).toBe("true");
  expect(screen.queryByText(/Project/)).toBeNull();
  workspace.dispose();
});

test("dataset selection clears old labels immediately and renders names only as text", async () => {
  const { Workspace } = await import("./workspace");
  const { Client } = await import("./api/client");
  const { fixtureTransport, datasetId } = await import("./test-fixtures");
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const user = userEvent.setup();
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("button", { name: "Catalogue" }));
  await user.click(await screen.findByRole("button", { name: /raw\/example/ }));
  await vi.waitFor(() =>
    expect(workspace.snapshot().selection.dataset).toBe(datasetId),
  );
  await screen.findByRole("button", { name: "1 node selected" });
  fireEvent.click(screen.getByRole("button", { name: "Properties" }));
  expect((await screen.findByText("Created")).nextSibling?.textContent).toMatch(
    /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/,
  );
  expect(screen.queryByText("Resolved head branch")).toBeNull();
  expect(screen.queryByText("Version context")).toBeNull();
  expect(screen.queryByRole("table")).toBeNull();
  workspace.dispose();
});

test("multiple graph nodes hide every bottom tab and right properties", async () => {
  const { Workspace } = await import("./workspace");
  const { Client } = await import("./api/client");
  const { fixtureTransport, datasetId, versionId, workspaceId, json, context } =
    await import("./test-fixtures");
  const nodes = [datasetId, versionId].map((id, index) => ({
    identity: `dataset:${workspaceId}:${id}`,
    paths: [index === 0 ? "raw/example" : "raw/other"],
    depth: "0",
    external: false,
    producer: true,
    resource_type: "polars_transform",
    publication: "published",
    parent_count: "0",
    child_count: "0",
  }));
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, query) =>
        path.endsWith("/lineage")
          ? Promise.resolve(
              json(
                {
                  nodes,
                  edges: [],
                  total_nodes: 2,
                  total_edges: 0,
                  next_cursor: null,
                  remaining_nodes: 0,
                  remaining_edges: 0,
                  omitted_nodes: 0,
                  omitted_edges: 0,
                  scope_complete: true,
                  external_expanded: false,
                },
                context(query.branch),
              ),
            )
          : undefined,
      ),
    ),
  );
  await workspace.connect("launch");
  const view = render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("button", { name: "Catalogue" }));
  fireEvent.click(await screen.findByRole("button", { name: /raw\/example/ }));
  await vi.waitFor(() =>
    expect(view.container.querySelectorAll(".react-flow__node")).toHaveLength(
      2,
    ),
  );
  fireEvent.click(screen.getByRole("button", { name: "Properties" }));
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  const diagram = screen.getByLabelText("Dataset lineage diagram");
  diagram.focus();
  fireEvent.keyDown(diagram, { key: "a", metaKey: true });
  expect(screen.getByRole("button", { name: "2 nodes selected" })).toBeTruthy();
  expect(screen.getAllByText("Select a node to view information")).toHaveLength(
    2,
  );
  expect(screen.queryByRole("table")).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: "History" }));
  expect(screen.getAllByText("Select a node to view information")).toHaveLength(
    2,
  );
  expect(
    screen.queryByText(
      "History content will be available in a later inspector task.",
    ),
  ).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  const label = view.container.querySelector(
    `.react-flow__node[data-id="dataset:${workspaceId}:${datasetId}"] .node-label`,
  );
  if (!label) throw new Error("Missing selected node label");
  fireEvent.click(label);
  expect(screen.getByRole("button", { name: "1 node selected" })).toBeTruthy();
  expect(screen.queryByText("Select a node to view information")).toBeNull();
  view.unmount();
  workspace.dispose();
});

test("historical context omits current-head details from the compact properties panel", async () => {
  const { Workspace } = await import("./workspace");
  const { Client } = await import("./api/client");
  const { fixtureTransport, datasetId, workspaceId, versionId } =
    await import("./test-fixtures");
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
    version: versionId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("button", { name: "Properties" }));
  expect(screen.queryByText("Published head")).toBeNull();
  expect(screen.queryByText("No published head")).toBeNull();
  expect(screen.queryByText("Producing source capture")).toBeNull();
  workspace.dispose();
});

test("panels default closed and fallback order saves a view-local override", async () => {
  const { Workspace } = await import("./workspace");
  const { Client } = await import("./api/client");
  const { fixtureTransport } = await import("./test-fixtures");
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const user = userEvent.setup();
  render(<App workspace={workspace} />);
  expect(screen.queryByRole("complementary")).toBeNull();
  expect(screen.queryByRole("tabpanel")).toBeNull();
  expect(screen.getByRole("button", { name: "Branch: master" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Apply branch" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Fallback branches" }));
  const fallbackDialog = screen.getByRole("dialog", {
    name: "Fallback branches",
  });
  expect(fallbackDialog.querySelectorAll(".fallback-list li")).toHaveLength(1);
  expect(fallbackDialog.querySelector(".fallback-name .icon")).toBeTruthy();
  await user.selectOptions(
    screen.getByLabelText("Add fallback branch"),
    "feature",
  );
  expect(fallbackDialog.querySelectorAll(".fallback-list li")).toHaveLength(2);
  await user.click(screen.getByRole("button", { name: "Move feature up" }));
  await user.click(screen.getByRole("button", { name: "Save and close" }));
  await vi.waitFor(() => expect(workspace.snapshot().kind).toBe("ready"));
  expect(workspace.snapshot().selection.fallback).toEqual([
    "feature",
    "public",
  ]);
  await user.click(screen.getByRole("button", { name: "Fallback branches" }));
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(workspace.snapshot().selection.fallback).toEqual([
    "feature",
    "public",
  ]);
  workspace.dispose();
});

test("branch dropdown searches the branches with heads for visible datasets", async () => {
  const { BranchControls } = await import("./BranchControls");
  const { Workspace } = await import("./workspace");
  const { Client } = await import("./api/client");
  const { fixtureTransport, json, context, workspaceId, datasetId } =
    await import("./test-fixtures");
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (path.endsWith("/branches") && q.datasets)
          return Promise.resolve(
            json(
              { entries: [{ name: "visible-only" }], next_cursor: null },
              context(q.branch),
            ),
          );
      }),
    ),
  );
  await workspace.connect("launch");
  const user = userEvent.setup();
  render(
    <BranchControls
      workspace={workspace}
      state={workspace.snapshot()}
      visible={[`dataset:${workspaceId}:${datasetId}`]}
    />,
  );
  await user.click(screen.getByRole("button", { name: "Branch: master" }));
  expect(
    await screen.findByRole("button", { name: "visible-only" }),
  ).toBeTruthy();
  expect(screen.queryByRole("button", { name: "feature" })).toBeNull();
  workspace.dispose();
});
