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
  await user.click(screen.getByRole("button", { name: /raw\/example/ }));
  await vi.waitFor(() =>
    expect(workspace.snapshot().selection.dataset).toBe(datasetId),
  );
  expect(screen.getByText("public")).toBeTruthy();
  expect(
    screen.getByText("Not pinned — browsing retained metadata"),
  ).toBeTruthy();
  expect(screen.queryByRole("table")).toBeNull();
  workspace.dispose();
});

test("historical context never labels unqueried current head as missing data", async () => {
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
  fireEvent.click(screen.getByRole("button", { name: "Catalogue" }));
  expect(screen.getByText("Not queried in historical context")).toBeTruthy();
  expect(screen.queryByText("No published head")).toBeNull();
  expect(screen.getByText("Producing source capture")).toBeTruthy();
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
  await user.selectOptions(
    screen.getByLabelText("Add fallback branch"),
    "feature",
  );
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
