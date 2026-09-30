import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { forwardRef } from "react";
import { App } from "./App";
import { PropertiesInspector, versionMetadata } from "./inspectors";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import {
  context,
  dataset,
  datasetId,
  fixtureTransport,
  json,
  versionId,
  workspaceId,
} from "./test-fixtures";

vi.mock("@revolist/react-datagrid", () => ({
  RevoGrid: forwardRef<
    HTMLTableElement,
    {
      columns: { prop: string; name: string }[];
      source: Record<string, string>[];
    }
  >(function TestGrid({ columns, source }, ref) {
    return (
      <table ref={ref}>
        <thead>
          <tr>
            <th>#</th>
            {columns.map((column) => (
              <th key={column.prop}>{column.name}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {source.map((row, index) => (
            <tr key={index}>
              <th>{index + 1}</th>
              {columns.map((column) => (
                <td key={column.prop}>{row[column.prop]}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    );
  }),
}));

const schema = {
  format_version: 1,
  fields: [
    {
      name: "amount",
      logical_type: { type: "decimal", precision: 20, scale: 2 },
      nullable: false,
    },
    { name: "note", logical_type: { type: "string" }, nullable: true },
  ],
};
const version = {
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
  integrity_state: "verified",
  file_count: "1",
};
const exact = {
  ...context(),
  selection: { kind: "local_version", id: versionId, digest: "f".repeat(64) },
  fingerprint: "f".repeat(64),
};
const preview = {
  workspace: workspaceId,
  origin_workspace: workspaceId,
  dataset: datasetId,
  version: versionId,
  requested_branch: "master",
  resolved_branch: "public",
  published_us: "1700000000000000",
  source: workspaceId,
  schema,
  rows: [
    [
      {
        value: {
          type: "decimal",
          value: "123456789012345678.90",
          precision: 20,
          scale: 2,
        },
        truncated: false,
      },
      { value: { type: "null" }, truncated: false },
    ],
    [
      {
        value: { type: "decimal", value: "0.00", precision: 20, scale: 2 },
        truncated: false,
      },
      { value: null, truncated: true },
    ],
  ],
  next_cursor: null,
  physical_order: true,
  integrity: "projected_read",
};

function transport(previewResponse: unknown = preview) {
  const base = fixtureTransport((path, query) =>
    path.endsWith("/versions")
      ? Promise.resolve(
          json(
            { entries: [version], next_cursor: null },
            context(query.branch),
          ),
        )
      : undefined,
  );
  return vi.fn((url: RequestInfo | URL, init?: RequestInit) =>
    String(url).startsWith("/api/v1/previews?")
      ? Promise.resolve(json(previewResponse, exact))
      : base(url, init),
  );
}

test("properties and preview show exact metadata, typed cells and zero byte count", async () => {
  const fetcher = transport();
  const workspace = new Workspace(new Client(fetcher));
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("button", { name: "Properties" }));
  expect(screen.getByText("0", { selector: "dd" })).toBeTruthy();
  expect(screen.getByText("2", { selector: "dd" })).toBeTruthy();
  expect(screen.getByText("Created").nextSibling?.textContent).toMatch(
    /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/,
  );
  expect(screen.queryByText("Engine")).toBeNull();
  expect(screen.queryByText("Version context")).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: "Columns" }));
  expect(screen.getByText("decimal(20, 2)")).toBeTruthy();
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  await waitFor(() => expect(screen.getByRole("table")).toBeTruthy());
  expect(screen.getByText(/123456789012345678.90/)).toBeTruthy();
  expect(screen.getByText("NULL")).toBeTruthy();
  expect(screen.getByText("Truncated · value unavailable")).toBeTruthy();
  expect(
    screen.getByText(/Could not find data on current branch/),
  ).toBeTruthy();
  expect(screen.queryByText("Search columns")).toBeNull();
  expect(screen.queryByText("Physical order · exact version")).toBeNull();
  expect(screen.getByRole("columnheader", { name: "amount" })).toBeTruthy();
  const call = fetcher.mock.calls.find(([url]) =>
    String(url).startsWith("/api/v1/previews?"),
  );
  expect(call).toBeTruthy();
  expect(JSON.parse(String(call?.[1]?.body))).toMatchObject({
    dataset: datasetId,
    origin_workspace: workspaceId,
    version: versionId,
    columns: ["amount", "note"],
  });
  workspace.dispose();
});

test("preview uses a browse-only available head when requested and fallback branches have none", async () => {
  const base = fixtureTransport((path, query) => {
    if (path === `/api/v1/datasets/${datasetId}`)
      return Promise.resolve(
        json({ ...dataset, head: null }, context(query.branch)),
      );
    if (path.endsWith("/versions"))
      return Promise.resolve(
        json({ entries: [], next_cursor: null }, context(query.branch)),
      );
    if (path.endsWith("/inspection"))
      return Promise.resolve(
        json(
          {
            dataset: datasetId,
            origin_workspace: workspaceId,
            created_us: "1700000000000000",
            suggested_head: {
              version: versionId,
              branch: "feature",
              published_us: "1700000000000000",
              schema,
            },
          },
          context(query.branch),
        ),
      );
  });
  const fetcher = vi.fn((url: RequestInfo | URL, init?: RequestInit) =>
    String(url).startsWith("/api/v1/previews?")
      ? Promise.resolve(json({ ...preview, resolved_branch: "feature" }, exact))
      : base(url, init),
  );
  const workspace = new Workspace(new Client(fetcher));
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  await waitFor(() => expect(screen.getByRole("table")).toBeTruthy());
  expect(
    screen.getByText(/Could not find data on current branch/).textContent,
  ).toContain("feature");
  expect(screen.getByText("Published").textContent).toMatch(/^Published/);
  expect(screen.queryByText("Requested branch")).toBeNull();
  expect(screen.queryByText("Source")).toBeNull();
  const call = fetcher.mock.calls.find(([url]) =>
    String(url).startsWith("/api/v1/previews?"),
  );
  expect(JSON.parse(String(call?.[1]?.body))).toMatchObject({
    version: versionId,
    columns: ["amount", "note"],
  });
  workspace.dispose();
});

test("preview omits resolved branch when it matches the selected branch", async () => {
  const workspace = new Workspace(
    new Client(transport({ ...preview, resolved_branch: "master" })),
  );
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  await waitFor(() => expect(screen.getByRole("table")).toBeTruthy());
  expect(
    screen.queryByText(/Could not find data on current branch/),
  ).toBeNull();
  workspace.dispose();
});

test("mismatched exact preview fails closed without rendering rows", async () => {
  const workspace = new Workspace(
    new Client(transport({ ...preview, version: workspaceId })),
  );
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    expect.stringContaining("does not match"),
  );
  expect(screen.queryByRole("table")).toBeNull();
  workspace.dispose();
});

test("multi-selection reports unknown metrics separately", async () => {
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const node = (
    id: string,
    resource_type: "polars_transform" | "external",
  ) => ({
    identity: `dataset:${workspaceId}:${id}`,
    paths: [id],
    depth: "0",
    external: resource_type === "external",
    producer: resource_type === "polars_transform",
    parent_count: "0",
    child_count: "0",
    publication: "unknown" as const,
    resource_type,
  });
  render(
    <PropertiesInspector
      workspace={workspace}
      state={workspace.snapshot()}
      selected={[
        node(datasetId, "polars_transform"),
        node(versionId, "external"),
      ]}
    />,
  );
  expect(screen.getByText("2 datasets selected")).toBeTruthy();
  expect(screen.getByText("Unknown row counts").nextSibling?.textContent).toBe(
    "2",
  );
  workspace.dispose();
});

test("multi-selection sums known physical counts and preserves real zeros", async () => {
  const otherId = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, query) => {
        if (path === `/api/v1/datasets/${datasetId}`)
          return Promise.resolve(
            json(
              {
                ...dataset,
                head: { ...dataset.head, row_count: "0", byte_count: "10" },
              },
              context(query.branch),
            ),
          );
        if (path === `/api/v1/datasets/${otherId}`)
          return Promise.resolve(
            json(
              {
                ...dataset,
                dataset_id: otherId,
                head: { ...dataset.head, row_count: "3", byte_count: "0" },
              },
              context(query.branch),
            ),
          );
      }),
    ),
  );
  await workspace.connect("launch");
  const node = (id: string) => ({
    identity: `dataset:${workspaceId}:${id}`,
    paths: [id],
    depth: "0",
    external: false,
    producer: true,
    parent_count: "0",
    child_count: "0",
    publication: "published" as const,
    resource_type: "polars_transform" as const,
  });
  render(
    <PropertiesInspector
      workspace={workspace}
      state={workspace.snapshot()}
      selected={[node(datasetId), node(otherId)]}
    />,
  );
  await waitFor(() =>
    expect(screen.getByText("Known rows").nextSibling?.textContent).toBe("3"),
  );
  expect(screen.getByText("Known bytes").nextSibling?.textContent).toBe("10");
  expect(screen.getByText("Unknown row counts").nextSibling?.textContent).toBe(
    "0",
  );
  expect(screen.getByText("Unknown byte counts").nextSibling?.textContent).toBe(
    "0",
  );
  workspace.dispose();
});

test("foreign manifest contributes exact physical counts without asserting byte availability", () => {
  const foreign = {
    ...dataset,
    origin: "external" as const,
    kind: "external" as const,
    freshness: "unknown" as const,
    head: null,
  };
  const metadata = {
    manifest: {
      ...schema,
      logical_schema: schema,
      files: [
        { row_count: "0", byte_length: "0" },
        { row_count: "2", byte_length: "314" },
      ],
    },
    source: { id: workspaceId },
    published_us: "1700000000000000",
  };
  const result = versionMetadata(
    foreign,
    {
      entries: [
        {
          version: versionId,
          origin_workspace: workspaceId,
          dataset: datasetId,
          metadata,
          availability: "not_verified",
          origin: "external",
        },
      ],
      next_cursor: null,
    },
    versionId,
  );
  expect(result).toMatchObject({
    rows: "2",
    bytes: "314",
    files: "2",
    source: workspaceId,
  });
  expect(result.schema?.fields).toHaveLength(2);
  expect(
    versionMetadata(foreign, { entries: [], next_cursor: null }, versionId)
      .rows,
  ).toBeUndefined();
});

test("preview cursor keeps the same immutable version and appends bounded rows", async () => {
  const base = transport();
  const fetcher = vi.fn((url: RequestInfo | URL, init?: RequestInit) => {
    if (String(url).startsWith("/api/v1/previews?")) {
      const request = JSON.parse(String(init?.body)) as {
        cursor?: string;
        version: string;
      };
      expect(request.version).toBe(versionId);
      return Promise.resolve(
        json(
          {
            ...preview,
            rows: request.cursor
              ? preview.rows.slice(1)
              : preview.rows.slice(0, 1),
            next_cursor: request.cursor ? null : "opaque-next",
          },
          exact,
        ),
      );
    }
    return base(url, init);
  });
  const workspace = new Workspace(new Client(fetcher));
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  fireEvent.click(
    await screen.findByRole("button", { name: "Load next 100 rows" }),
  );
  await waitFor(() => expect(screen.getAllByRole("row")).toHaveLength(3));
  const requests = fetcher.mock.calls.filter(([url]) =>
    String(url).startsWith("/api/v1/previews?"),
  );
  expect(JSON.parse(String(requests[1]?.[1]?.body))).toMatchObject({
    cursor: "opaque-next",
    version: versionId,
  });
  workspace.dispose();
});

test("preview stops offering pages after 1000 displayed rows", async () => {
  const workspace = new Workspace(
    new Client(
      transport({
        ...preview,
        rows: Array.from({ length: 1000 }, () => preview.rows[0] ?? []),
        next_cursor: "more-rows",
      }),
    ),
  );
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  render(<App workspace={workspace} />);
  fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
  await waitFor(() =>
    expect(screen.getByText(/Showing 1000.*preview limit/)).toBeTruthy(),
  );
  expect(
    screen.queryByRole("button", { name: "Load next 100 rows" }),
  ).toBeNull();
  expect(screen.getAllByRole("row")).toHaveLength(1001);
  workspace.dispose();
});

test("late preview response cannot cross a workspace context change", async () => {
  const base = transport();
  let complete: ((value: Response) => void) | undefined;
  const fetcher: typeof fetch = (url, init) =>
    String(url).startsWith("/api/v1/previews?")
      ? new Promise<Response>((resolve) => {
          complete = resolve;
        })
      : base(url, init);
  const workspace = new Workspace(new Client(fetcher));
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  const pending = workspace.preview({
    dataset: datasetId,
    origin_workspace: workspaceId,
    version: versionId,
    columns: ["amount"],
  });
  expect(complete).toBeDefined();
  await workspace.select({ branch: "feature" });
  complete?.(json(preview, exact));
  await expect(pending).rejects.toHaveProperty("name", "AbortError");
  workspace.dispose();
});
