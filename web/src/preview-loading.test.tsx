import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, test } from "vitest";
import { App } from "./App";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import type {
  ApiQueryRequestV1,
  ApiQueryResultsV1,
} from "./generated/contracts";
import {
  context,
  datasetId,
  fixtureTransport,
  json,
  versionId,
  workspaceId,
} from "./test-fixtures";

const queryId = "11111111-1111-4111-8111-111111111111";
const schema: ApiQueryResultsV1["schema"] = {
  format_version: 1,
  fields: [{ name: "id", logical_type: { type: "i64" }, nullable: false }],
};
const physical = {
  workspace: workspaceId,
  origin_workspace: workspaceId,
  dataset: datasetId,
  version: versionId,
  requested_branch: "master",
  resolved_branch: "public",
  published_us: "1700000000000000",
  source: workspaceId,
  schema,
  rows: [1, 2].map((value) => [
    { value: { type: "i64", value: String(value) }, truncated: false },
  ]),
  next_cursor: null,
  physical_order: true,
  integrity: "projected_read",
};
async function fixture() {
  const c = context();
  const exact = {
    ...c,
    selection: { kind: "local_version", id: versionId, digest: "f".repeat(64) },
    fingerprint: "f".repeat(64),
  };
  let complete: ((response: Response) => void) | undefined;
  let request: ApiQueryRequestV1 | undefined;
  let holdPhysical = false;
  let completePhysical: ((response: Response) => void) | undefined;
  const base = fixtureTransport((path, query) => {
    if (path.endsWith("/versions"))
      return Promise.resolve(
        json(
          {
            entries: [
              {
                version: versionId,
                source: workspaceId,
                published_at_us: "1700000000000000",
                artifact: "a".repeat(64),
                schema,
                attempt: null,
                import: null,
                origin_workspace: workspaceId,
                dataset: datasetId,
                availability: "not_verified",
                origin: "local",
                row_count: "2",
                byte_count: "0",
                file_count: "1",
                integrity_state: "verified",
              },
            ],
            next_cursor: null,
          },
          context(query.branch),
        ),
      );
    if (path.endsWith("/results"))
      return new Promise<Response>((resolve) => {
        complete = resolve;
      });
    if (path === `/api/v1/queries/${queryId}`)
      return Promise.resolve(
        json(
          {
            ...request,
            id: queryId,
            state: "SUCCEEDED",
            elapsed_ms: "1",
            error: null,
            truncated: false,
            rows: "1",
            limits: {
              rows: 1000,
              bytes: 2097152,
              spill_bytes: "0",
              threads: 1,
              memory_bytes: null,
            },
            timeout_seconds: "30",
            expires_in_seconds: 300,
          },
          c,
        ),
      );
    return undefined;
  });
  const workspace = new Workspace(
    new Client(async (url, init) => {
      if (String(url).startsWith("/api/v1/previews?")) {
        if (holdPhysical)
          return new Promise<Response>((resolve) => {
            completePhysical = resolve;
          });
        return json(physical, exact);
      }
      if (String(url).startsWith("/api/v1/queries?")) {
        request = JSON.parse(String(init?.body)) as ApiQueryRequestV1;
        return json(
          {
            ...request,
            id: queryId,
            state: "QUEUED",
            elapsed_ms: "0",
            error: null,
            truncated: false,
            rows: "0",
            limits: {
              rows: 1000,
              bytes: 2097152,
              spill_bytes: "0",
              threads: 1,
              memory_bytes: null,
            },
            timeout_seconds: "30",
            expires_in_seconds: 300,
          },
          c,
        );
      }
      return base(url, init);
    }),
  );
  await workspace.connect("launch");
  const view = render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("button", { name: "Catalogue" }));
  fireEvent.click(await screen.findByRole("button", { name: /raw\/example/ }));
  await screen.findByRole("button", { name: "1 node selected" });
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  await screen.findByRole("table");
  return {
    view,
    workspace,
    holdPhysical: () => {
      holdPhysical = true;
    },
    finishPhysical: async () => {
      await waitFor(() => expect(completePhysical).toBeDefined());
      await act(async () => completePhysical?.(json(physical, exact)));
    },
    pending: async () => {
      await waitFor(() => expect(complete).toBeDefined());
    },
    finish: async (wrongId = false, empty = false) => {
      await waitFor(() => expect(complete).toBeDefined());
      const resolve = complete;
      complete = undefined;
      await act(async () =>
        resolve?.(
          json(
            {
              id: wrongId ? datasetId : queryId,
              schema,
              rows: empty ? [] : [[{ type: "i64", value: "2" }]],
              next_offset: null,
              truncated: false,
            },
            c,
          ),
        ),
      );
    },
  };
}

test.each(["sort", "filter"])(
  "%s retains the exact table disabled beneath a corner spinner until replacement arrives",
  async (action) => {
    const f = await fixture();
    const grid = f.view.container.querySelector(".preview-grid-wrap");
    const originalTable = screen.getByRole("table");
    fireEvent.click(
      screen.getByRole("button", { name: "Column options for id" }),
    );
    if (action === "sort")
      fireEvent.click(screen.getByRole("button", { name: "Sort descending" }));
    else {
      fireEvent.change(
        screen.getByRole("textbox", { name: "Filter value for id" }),
        { target: { value: "999" } },
      );
      fireEvent.click(screen.getByRole("button", { name: "Apply id filter" }));
    }
    expect(
      screen.getByRole("status", { name: "Updating preview" }),
    ).toBeTruthy();
    expect(grid?.getAttribute("aria-busy")).toBe("true");
    expect(grid?.getAttribute("aria-disabled")).toBe("true");
    expect(
      grid?.querySelector(".preview-grid-content")?.hasAttribute("inert"),
    ).toBe(true);
    expect(grid?.querySelector("table")).toBe(originalTable);
    expect(originalTable.textContent).toContain("1");
    expect(grid?.querySelector(".preview-grid-shade")).toBeTruthy();
    expect(screen.queryByText("Sorted preview")).toBeNull();
    expect(screen.queryByText("Updating preview…")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Reset preview" }).parentElement
        ?.className,
    ).toBe("dataset-inspector-leading");
    await f.finish(false, action === "filter");
    const resultRows = action === "filter" ? 0 : 1;
    expect(screen.queryByText("Filtered preview")).toBeNull();
    expect(screen.queryByText("No rows match this preview.")).toBeNull();
    await waitFor(() => expect(grid?.getAttribute("aria-busy")).toBe("false"));
    expect(grid?.getAttribute("aria-disabled")).toBe("false");
    expect(
      grid?.querySelector(".preview-grid-content")?.hasAttribute("inert"),
    ).toBe(false);
    expect(
      screen.queryByRole("status", { name: "Updating preview" }),
    ).toBeNull();
    expect(originalTable.querySelectorAll("tbody tr")).toHaveLength(resultRows);
    f.holdPhysical();
    fireEvent.click(screen.getByRole("button", { name: "Reset preview" }));
    expect(
      screen.getByRole("status", { name: "Updating preview" }),
    ).toBeTruthy();
    expect(originalTable.querySelectorAll("tbody tr")).toHaveLength(resultRows);
    await f.finishPhysical();
    await waitFor(() =>
      expect(originalTable.querySelectorAll("tbody tr")).toHaveLength(2),
    );
    expect(screen.queryByRole("button", { name: "Reset preview" })).toBeNull();
    f.view.unmount();
    f.workspace.dispose();
  },
);

test("a rejected replacement stops the spinner and keeps previous rows visibly disabled until reset", async () => {
  const f = await fixture();
  fireEvent.click(
    screen.getByRole("button", { name: "Column options for id" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Sort ascending" }));
  await f.finish(true);
  expect((await screen.findByRole("alert")).textContent).toMatch(
    /Previous rows are shown disabled/,
  );
  expect(screen.queryByRole("status", { name: "Updating preview" })).toBeNull();
  const grid = f.view.container.querySelector(".preview-grid-wrap");
  expect(grid?.getAttribute("aria-disabled")).toBe("true");
  expect(grid?.querySelectorAll("tbody tr")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "Restart preview" }));
  expect(screen.getByRole("status", { name: "Updating preview" })).toBeTruthy();
  expect(grid?.querySelectorAll("tbody tr")).toHaveLength(2);
  await f.finish();
  await waitFor(() =>
    expect(grid?.getAttribute("aria-disabled")).toBe("false"),
  );
  expect(grid?.querySelectorAll("tbody tr")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Reset preview" }));
  await waitFor(() =>
    expect(grid?.getAttribute("aria-disabled")).toBe("false"),
  );
  expect(screen.queryByRole("alert")).toBeNull();
  f.view.unmount();
  f.workspace.dispose();
});

test("changing branch discards the retained table and a late query page cannot restore it", async () => {
  const f = await fixture();
  fireEvent.click(
    screen.getByRole("button", { name: "Column options for id" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Sort descending" }));
  await f.pending();
  expect(screen.getByRole("status", { name: "Updating preview" })).toBeTruthy();
  f.holdPhysical();
  await act(async () => {
    await f.workspace.select({ branch: "feature" });
  });
  expect(f.view.container.querySelector(".preview-grid-wrap")).toBeNull();
  await f.finish();
  expect(f.view.container.querySelector(".preview-grid-wrap")).toBeNull();
  expect(screen.queryByRole("status", { name: "Updating preview" })).toBeNull();
  f.view.unmount();
  f.workspace.dispose();
});
