import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import { ScratchpadInspector } from "./ScratchpadInspector";
import { HealthInspector } from "./HealthInspector";
import { PropertiesInspector } from "./inspectors";
import { DatasetTooltip } from "./graph/Tooltip";
import type { Ready } from "./execution-ui";
import type { ApiQueryV1, ApiLineageNodeV1 } from "./generated/contracts";
import {
  context,
  dataset,
  datasetId,
  fixtureTransport,
  json,
  versionId,
  workspaceId,
} from "./test-fixtures";

const queryId = "11111111-1111-4111-8111-111111111111";
const publishedAttempt = "22222222-2222-4222-8222-222222222222";
const failedAttempt = "33333333-3333-4333-8333-333333333333";
const planContext = () => ({
  ...context(),
  selection: { kind: "plan" as const, id: queryId, digest: "f".repeat(64) },
  fingerprint: "f".repeat(64),
});
const schema = {
  format_version: 1,
  fields: [
    {
      name: "amount",
      logical_type: { type: "i64" },
      nullable: true,
    },
  ],
};
const version = {
  version: versionId,
  source: workspaceId,
  published_at_us: "1700000000000000",
  artifact: "a".repeat(64),
  schema,
  attempt: publishedAttempt,
  import: null,
  origin_workspace: workspaceId,
  dataset: datasetId,
  availability: "not_verified",
  origin: "local",
  row_count: "2",
  byte_count: "42",
  integrity_state: "VERIFIED",
};
const accepted: ApiQueryV1 = {
  id: queryId,
  state: "QUEUED",
  bindings: [
    {
      alias: "dataset",
      dataset: datasetId,
      origin_workspace: workspaceId,
      version: versionId,
    },
  ],
  sql: 'SELECT * FROM "dataset"',
  parameters: [],
  elapsed_ms: "0",
  error: null,
  truncated: false,
  rows: "0",
  limits: {
    rows: 1000,
    bytes: 1048576,
    spill_bytes: "0",
    threads: 1,
    memory_bytes: null,
  },
  timeout_seconds: "30",
  expires_in_seconds: null,
};
type Reader = Parameters<typeof fixtureTransport>[0];
async function setup(
  read?: Reader,
  post?: (url: string, init?: RequestInit) => Promise<Response> | undefined,
) {
  const base = fixtureTransport((path, query, signal) => {
    if (path.endsWith("/context") && query.plan)
      return Promise.resolve(json(planContext(), planContext()));
    const override = read?.(path, query, signal);
    if (override) return override;
    if (path.endsWith("/versions"))
      return Promise.resolve(
        json({ entries: [version], next_cursor: null }, context(query.branch)),
      );
    return undefined;
  });
  const transport = vi.fn(
    (url: RequestInfo | URL, init?: RequestInit) =>
      post?.(String(url), init) ?? base(url, init),
  );
  const workspace = new Workspace(new Client(transport));
  await workspace.connect("launch");
  await workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  const state = workspace.snapshot();
  if (state.kind !== "ready") throw new Error("Fixture did not become ready");
  return { workspace, state, transport };
}
function successful(path: string) {
  if (path.endsWith(`/queries/${queryId}`))
    return Promise.resolve(
      json(
        {
          ...accepted,
          state: "SUCCEEDED",
          rows: "2",
          elapsed_ms: "13",
          expires_in_seconds: 300,
        },
        context(),
      ),
    );
  if (path.endsWith("/results"))
    return Promise.resolve(
      json(
        {
          id: queryId,
          schema,
          rows: [
            [{ type: "i64", value: "9007199254740993" }],
            [{ type: "null" }],
          ],
          next_offset: null,
          truncated: false,
        },
        context(),
      ),
    );
}
test("scratchpad shows the editor and empty results without parameter controls", async () => {
  const { workspace, state } = await setup();
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  const editor = screen.getByRole("region", { name: "SQL editor" });
  const results = screen.getByRole("region", { name: "SQL results" });
  expect(within(editor).getByLabelText("SQL query")).toBeTruthy();
  expect(
    within(results).queryByRole("heading", { name: "Results" }),
  ).toBeNull();
  expect(
    within(results).getByText(
      "Run a SQL query in the editor to see a preview of the results.",
    ),
  ).toBeTruthy();
  expect(screen.queryByText("SQL scratchpad")).toBeNull();
  expect(
    screen.queryByText("Query the selected published version as dataset."),
  ).toBeNull();
  expect(within(editor).queryByText("Parameters")).toBeNull();
  expect(
    within(editor).queryByLabelText("Bound values (JSON array)"),
  ).toBeNull();
  expect(screen.queryByLabelText("Query running")).toBeNull();
  workspace.dispose();
});

test("an unpublished dataset retains its header and results pane without starting a query", async () => {
  const { workspace, state, transport } = await setup();
  if (!state.value.dataset) throw new Error("fixture dataset missing");
  const unpublished: Ready = {
    ...state,
    value: {
      ...state.value,
      dataset: { ...state.value.dataset, head: null },
      versions: { entries: [], next_cursor: null },
      inspection: null,
    },
  };
  render(
    <ScratchpadInspector
      workspace={workspace}
      state={unpublished}
      dark={false}
    />,
  );
  expect(screen.getByRole("heading", { name: dataset.path })).toBeTruthy();
  expect(screen.getByRole("region", { name: "SQL results" })).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "Run query" }).hasAttribute("disabled"),
  ).toBe(true);
  expect(
    screen.getByText("No published version is available for this query."),
  ).toBeTruthy();
  expect(
    transport.mock.calls.some(([url]) =>
      String(url).startsWith("/api/v1/queries?"),
    ),
  ).toBe(false);
  workspace.dispose();
});

test("scratchpad binds the displayed version and renders exact typed result cells", async () => {
  const { workspace, state, transport } = await setup(successful, (url) =>
    url.startsWith("/api/v1/queries?")
      ? Promise.resolve(json(accepted, context()))
      : undefined,
  );
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByText("9007199254740993");
  expect(
    screen.getByText(/Could not find data on current branch/),
  ).toBeTruthy();
  expect(screen.getByText("NULL")).toBeTruthy();
  const call = transport.mock.calls.find(([url]) =>
    String(url).startsWith("/api/v1/queries?"),
  );
  expect(JSON.parse(String(call?.[1]?.body)).bindings).toEqual(
    accepted.bindings,
  );
  expect(JSON.parse(String(call?.[1]?.body)).parameters).toEqual([]);
  expect(
    screen.getByRole("status", { name: "Query status" }).textContent,
  ).not.toContain("SUCCEEDED");
  expect(screen.queryByText(/time limit/)).toBeNull();
  expect(screen.getByRole("button", { name: "Download TSV" })).toBeTruthy();
  expect(
    new URL(String(call?.[0]), "http://test").searchParams.has("version"),
  ).toBe(false);
  expect(screen.queryByRole("alert")).toBeNull();
  workspace.dispose();
});
test("a delayed old result page cannot replace a newly run query", async () => {
  let complete!: (response: Response) => void;
  const delayed = new Promise<Response>((resolve) => {
    complete = resolve;
  });
  let starts = 0;
  const nextSql = "SELECT 42 AS amount FROM dataset LIMIT 1";
  const { workspace, state, transport } = await setup(
    (path, query) => {
      if (path.endsWith(`/queries/${workspaceId}`))
        return Promise.resolve(
          json(
            {
              ...accepted,
              id: workspaceId,
              sql: nextSql,
              state: "SUCCEEDED",
              rows: "1",
            },
            context(),
          ),
        );
      if (path.endsWith("/results")) {
        if (query.offset === "1") return delayed;
        const next = path.includes(workspaceId);
        return Promise.resolve(
          json(
            {
              id: next ? workspaceId : queryId,
              schema,
              rows: [[{ type: "i64", value: next ? "42" : "1" }]],
              next_offset: next ? null : 1,
              truncated: false,
            },
            context(),
          ),
        );
      }
      return successful(path);
    },
    (url) => {
      if (!url.startsWith("/api/v1/queries?")) return;
      starts += 1;
      return Promise.resolve(
        json(
          starts === 1
            ? accepted
            : { ...accepted, id: workspaceId, sql: nextSql },
          context(),
        ),
      );
    },
  );
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByRole("table", { name: "SQL query results data grid" });
  fireEvent.wheel(screen.getByLabelText("SQL query results preview"), {
    deltaY: 100,
  });
  await waitFor(() =>
    expect(
      transport.mock.calls.some(([, init]) =>
        String(init?.body).includes('"offset":"1"'),
      ),
    ).toBe(true),
  );
  fireEvent.change(screen.getByLabelText("SQL query"), {
    target: { value: nextSql },
  });
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByText("42");
  await act(async () =>
    complete(
      json(
        {
          id: queryId,
          schema,
          rows: [[{ type: "i64", value: "99" }]],
          next_offset: null,
          truncated: false,
        },
        context(),
      ),
    ),
  );
  expect(screen.getByText("42")).toBeTruthy();
  expect(screen.queryByText("99")).toBeNull();
  workspace.dispose();
});

test("empty SQL does not start a query", async () => {
  const { workspace, state, transport } = await setup();
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.change(screen.getByLabelText("SQL query"), {
    target: { value: "  " },
  });
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  expect(
    screen.getByRole("button", { name: "Run query" }).hasAttribute("disabled"),
  ).toBe(true);
  expect(
    transport.mock.calls.some(([url]) =>
      String(url).startsWith("/api/v1/queries?"),
    ),
  ).toBe(false);
  workspace.dispose();
});

test.each(["SUCCEEDED", "FAILED", "CANCELED", "TIMED_OUT"] as const)(
  "query progress is visible while pending and stops on %s",
  async (stateName) => {
    let finish!: (value: Response) => void;
    const pending = new Promise<Response>((resolve) => {
      finish = resolve;
    });
    let statusStarted = false;
    const { workspace, state } = await setup(
      (path) => {
        if (path.endsWith(`/queries/${queryId}`)) {
          statusStarted = true;
          return pending;
        }
        return successful(path);
      },
      (url) =>
        url.startsWith("/api/v1/queries?")
          ? Promise.resolve(json(accepted, context()))
          : undefined,
    );
    render(
      <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Run query" }));
    expect(screen.getByLabelText("Query running")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Cancel query" }).nextElementSibling,
    ).toBe(screen.getByLabelText("Query running"));
    await waitFor(() => expect(statusStarted).toBe(true));
    await act(async () =>
      finish(
        json(
          {
            ...accepted,
            state: stateName,
            rows: stateName === "SUCCEEDED" ? "2" : "0",
            error:
              stateName === "FAILED" || stateName === "TIMED_OUT"
                ? 'Binder Error: Referenced column "ida" not found.\nLINE 1: SELECT * FROM dataset WHERE ida >= 2 <img src=x>'
                : null,
          },
          context(),
        ),
      ),
    );
    await waitFor(() =>
      expect(screen.queryByLabelText("Query running")).toBeNull(),
    );
    expect(
      screen
        .getByRole("button", { name: "Run query" })
        .hasAttribute("disabled"),
    ).toBe(false);
    if (stateName === "FAILED" || stateName === "TIMED_OUT") {
      expect(screen.getByRole("alert").textContent).toContain(
        'Referenced column "ida" not found.\nLINE 1:',
      );
      expect(screen.getByRole("alert").querySelector("img")).toBeNull();
      expect(
        screen.queryByText(
          "Run a SQL query in the editor to see a preview of the results.",
        ),
      ).toBeNull();
    }
    workspace.dispose();
  },
);

function mockDownload() {
  const blobs: Blob[] = [];
  URL.createObjectURL = vi.fn((blob: Blob | MediaSource) => {
    if (blob instanceof Blob) blobs.push(blob);
    return "blob:query-results";
  });
  URL.revokeObjectURL = vi.fn();
  const click = vi
    .spyOn(HTMLAnchorElement.prototype, "click")
    .mockImplementation(() => undefined);
  return { blobs, click };
}
function blobText(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });
}

test("download includes headers and every result page, then releases its URL on exit", async () => {
  const saved = mockDownload();
  const { workspace, state } = await setup(
    (path, query) => {
      if (path.endsWith(`/queries/${queryId}`))
        return Promise.resolve(
          json({ ...accepted, state: "SUCCEEDED", rows: "3" }, context()),
        );
      if (path.endsWith("/results"))
        return Promise.resolve(
          json(
            {
              id: queryId,
              schema,
              rows:
                query.offset === "1"
                  ? [
                      [{ type: "i64", value: "2" }],
                      [{ type: "i64", value: "3" }],
                    ]
                  : [[{ type: "i64", value: "1" }]],
              next_offset: query.offset === "1" ? null : 1,
              truncated: false,
            },
            context(),
          ),
        );
      return undefined;
    },
    (url) =>
      url.startsWith("/api/v1/queries?")
        ? Promise.resolve(json(accepted, context()))
        : undefined,
  );
  const view = render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  fireEvent.click(await screen.findByRole("button", { name: "Download TSV" }));
  await waitFor(() => expect(saved.click).toHaveBeenCalledOnce());
  const blob = saved.blobs[0];
  if (!blob) throw new Error("Missing download blob");
  expect(blob.type).toBe("text/tab-separated-values;charset=utf-8");
  expect(await blobText(blob)).toBe("amount\n1\n2\n3");
  view.unmount();
  expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:query-results");
  workspace.dispose();
});

test.each(["expired", "schema", "identity", "incomplete"])(
  "download does not save a partial file after %s results",
  async (kind) => {
    const saved = mockDownload();
    const { workspace, state } = await setup(
      (path, query) => {
        if (path.endsWith(`/queries/${queryId}`))
          return Promise.resolve(
            json({ ...accepted, state: "SUCCEEDED", rows: "2" }, context()),
          );
        if (path.endsWith("/results")) {
          if (query.offset === "1")
            return Promise.resolve(
              kind === "expired"
                ? new Response(
                    JSON.stringify({ error: { message: "Results expired" } }),
                    { status: 410 },
                  )
                : json(
                    {
                      id: kind === "identity" ? workspaceId : queryId,
                      schema:
                        kind === "schema" ? { ...schema, fields: [] } : schema,
                      rows:
                        kind === "schema" || kind === "incomplete"
                          ? []
                          : [[{ type: "i64", value: "2" }]],
                      next_offset: null,
                      truncated: false,
                    },
                    context(),
                  ),
            );
          return Promise.resolve(
            json(
              {
                id: queryId,
                schema,
                rows: [[{ type: "i64", value: "1" }]],
                next_offset: 1,
                truncated: false,
              },
              context(),
            ),
          );
        }
        return undefined;
      },
      (url) =>
        url.startsWith("/api/v1/queries?")
          ? Promise.resolve(json(accepted, context()))
          : undefined,
    );
    const view = render(
      <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Run query" }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Download TSV" }),
    );
    await screen.findByRole("alert");
    expect(saved.blobs).toHaveLength(0);
    expect(saved.click).not.toHaveBeenCalled();
    view.unmount();
    workspace.dispose();
  },
);

test("a late download page after selection exit cannot save a file", async () => {
  const saved = mockDownload();
  let finish!: (value: Response) => void;
  const pending = new Promise<Response>((resolve) => {
    finish = resolve;
  });
  const { workspace, state, transport } = await setup(
    (path, query) => {
      if (path.endsWith("/results"))
        return query.offset === "1"
          ? pending
          : Promise.resolve(
              json(
                {
                  id: queryId,
                  schema,
                  rows: [[{ type: "i64", value: "1" }]],
                  next_offset: 1,
                  truncated: false,
                },
                context(),
              ),
            );
      return successful(path);
    },
    (url) =>
      url.startsWith("/api/v1/queries?")
        ? Promise.resolve(json(accepted, context()))
        : undefined,
  );
  const view = render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  fireEvent.click(await screen.findByRole("button", { name: "Download TSV" }));
  await waitFor(() =>
    expect(
      transport.mock.calls.some(([, init]) =>
        String(init?.body).includes('"offset":"1"'),
      ),
    ).toBe(true),
  );
  view.unmount();
  await act(async () =>
    finish(
      json(
        {
          id: queryId,
          schema,
          rows: [[{ type: "i64", value: "2" }]],
          next_offset: null,
          truncated: false,
        },
        context(),
      ),
    ),
  );
  expect(saved.click).not.toHaveBeenCalled();
  workspace.dispose();
});
test("an accepted query arriving after unmount is canceled with its original context", async () => {
  let complete!: (value: Response) => void;
  const pending = new Promise<Response>((resolve) => {
    complete = resolve;
  });
  const { workspace, state, transport } = await setup(undefined, (url) =>
    url.startsWith("/api/v1/queries?")
      ? pending
      : url.includes("/cancel?")
        ? Promise.resolve(json({ ...accepted, state: "CANCELED" }, context()))
        : undefined,
  );
  const view = render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await waitFor(() =>
    expect(
      transport.mock.calls.some(([url]) =>
        String(url).startsWith("/api/v1/queries?"),
      ),
    ).toBe(true),
  );
  view.unmount();
  await act(async () => {
    complete(json(accepted, context()));
  });
  await waitFor(() =>
    expect(
      transport.mock.calls.filter(([url]) => String(url).includes("/cancel?")),
    ).toHaveLength(1),
  );
  expect(
    String(
      transport.mock.calls.find(([url]) =>
        String(url).includes("/cancel?"),
      )?.[0],
    ),
  ).toContain(`context=${context().fingerprint}`);
  workspace.dispose();
});
test("cancel before acceptance remains pending and does not create a second query", async () => {
  let complete!: (value: Response) => void;
  const pending = new Promise<Response>((resolve) => {
    complete = resolve;
  });
  const { workspace, state, transport } = await setup(
    (path) =>
      path.endsWith(`/queries/${queryId}`)
        ? Promise.resolve(json({ ...accepted, state: "CANCELED" }, context()))
        : undefined,
    (url) =>
      url.startsWith("/api/v1/queries?")
        ? pending
        : url.includes("/cancel?")
          ? Promise.resolve(json({ ...accepted, state: "CANCELED" }, context()))
          : undefined,
  );
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await waitFor(() =>
    expect(
      transport.mock.calls.some(([url]) =>
        String(url).startsWith("/api/v1/queries?"),
      ),
    ).toBe(true),
  );
  fireEvent.click(screen.getByRole("button", { name: "Cancel query" }));
  expect(
    (screen.getByRole("button", { name: "Canceling…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  await act(async () => {
    complete(json(accepted, context()));
  });
  await screen.findByText(/^CANCELED ·/);
  expect(screen.queryByRole("alert")).toBeNull();
  expect(
    transport.mock.calls.filter(([url]) =>
      String(url).startsWith("/api/v1/queries?"),
    ),
  ).toHaveLength(1);
  workspace.dispose();
});
test.each(["binding", "page", "expired"])(
  "scratchpad rejects %s mismatch or expiry without fabricated rows",
  async (kind) => {
    const { workspace, state } = await setup(
      (path) => {
        if (path.endsWith("/results"))
          return Promise.resolve(
            kind === "expired"
              ? new Response(
                  JSON.stringify({ error: { message: "Results expired" } }),
                  { status: 410 },
                )
              : json(
                  {
                    id: workspaceId,
                    schema,
                    rows: [],
                    next_offset: null,
                    truncated: false,
                  },
                  context(),
                ),
          );
        return successful(path);
      },
      (url) =>
        url.startsWith("/api/v1/queries?")
          ? Promise.resolve(
              json(
                kind === "binding"
                  ? {
                      ...accepted,
                      bindings: [
                        { ...accepted.bindings[0], version: workspaceId },
                      ],
                    }
                  : accepted,
                context(),
              ),
            )
          : undefined,
    );
    render(
      <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Run query" }));
    await screen.findByRole("alert");
    expect(screen.queryByRole("table")).toBeNull();
    expect(
      (screen.getByRole("button", { name: "Run query" }) as HTMLButtonElement)
        .disabled,
    ).toBe(false);
    workspace.dispose();
  },
);
const node: ApiLineageNodeV1 = {
  identity: `dataset:${workspaceId}:${datasetId}`,
  paths: [dataset.path],
  depth: "0",
  external: false,
  producer: true,
  parent_count: "0",
  child_count: "0",
  publication: "published",
  resource_type: "polars_transform",
  overlay: {
    version: versionId,
    rows: "2",
    files: "1",
    bytes: "42",
    latest_attempt: "Failed",
    freshness: "current",
    reasons: [],
    health: "Passed",
    input_failed: true,
    durations: null,
  },
};
function attempt(id: string, phase: string, outcome: string) {
  return {
    attempt: id,
    build: queryId,
    job: queryId,
    dataset: datasetId,
    plan: queryId,
    source: workspaceId,
    source_availability: "retained",
    parameters: {},
    inputs: [],
    duration_ns: "10",
    log_command: "transflow logs",
    evidence: {
      state: id === publishedAttempt ? "SUCCEEDED" : "FAILED",
      started_us: "1700000000000000",
      checks: [
        {
          id: queryId,
          name: "<img src=x onerror=alert(1)>",
          phase,
          alias: phase === "input" ? "rows" : null,
          outcome,
          policy: { severity: "FAIL" },
          metrics: { failed_rows: "0" },
        },
      ],
    },
  };
}
function history() {
  return {
    entries: [
      {
        id: queryId,
        build: queryId,
        plan: queryId,
        source: workspaceId,
        state: "FAILED",
        created_us: "1700000000000000",
        finished_us: "1700000000000010",
        attempt_count: "1",
        produced_version: null,
        reused_version: null,
        original_attempt: null,
        duration_ns: "10",
        build_state: "FAILED",
        build_job_count: "1",
        attempts: [
          {
            attempt: failedAttempt,
            number: "1",
            state: "FAILED",
            started_us: "1700000000000000",
            finished_us: "1700000000000010",
            duration_ns: "10",
            phases: [],
          },
        ],
      },
    ],
    next_cursor: null,
  };
}
test("Properties shows publication freshness and ten read-only output-check circles", async () => {
  const { workspace, state, transport } = await setup((path, query) =>
    path.endsWith("/history")
      ? Promise.resolve(json(history(), context(query.branch)))
      : path.endsWith(`/attempts/${publishedAttempt}`)
        ? Promise.resolve(
            json(attempt(publishedAttempt, "output", "PASS"), {
              ...context(query.branch),
              selection: {
                kind: "local_version",
                id: versionId,
                digest: "f".repeat(64),
              },
              fingerprint: "f".repeat(64),
            }),
          )
        : undefined,
  );
  if (!node.overlay) throw new Error("Fixture overlay is missing");
  const selectedNode = {
    ...node,
    overlay: { ...node.overlay, data_freshness: "stale" as const },
  };
  const selected = [selectedNode];
  const view = render(
    <PropertiesInspector
      workspace={workspace}
      state={state}
      selected={selected}
    />,
  );
  expect(screen.getByRole("heading", { name: "Freshness" })).toBeTruthy();
  expect(screen.getByText("Out-of-date")).toBeTruthy();
  expect(screen.queryByText(/Explore the lineage graph/)).toBeNull();
  expect(screen.queryByText(/Latest attempt:/)).toBeNull();
  const outputs = screen.getByRole("region", { name: "Output dataset checks" });
  await within(outputs).findByText("Passed");
  expect(within(outputs).getAllByRole("listitem")).toHaveLength(10);
  expect(within(outputs).getAllByRole("img")).toHaveLength(10);
  expect(within(outputs).queryByRole("button")).toBeNull();
  expect(outputs.querySelector(".health-history-details")).toBeNull();
  expect(screen.queryByRole("region", { name: "Latest builds" })).toBeNull();
  expect(
    screen.queryByRole("region", { name: "Input dataset checks" }),
  ).toBeNull();
  expect(
    transport.mock.calls.some(([, init]) =>
      String(init?.body).includes(`/attempts/${failedAttempt}`),
    ),
  ).toBe(false);
  view.rerender(
    <PropertiesInspector
      workspace={workspace}
      state={state}
      selected={[
        {
          ...selectedNode,
          overlay: { ...selectedNode.overlay, version: workspaceId },
        },
      ]}
    />,
  );
  expect(screen.queryByText("Out-of-date")).toBeNull();
  expect(
    screen.getByRole("heading", { name: "Freshness" }).nextElementSibling
      ?.textContent,
  ).toBe("Unavailable");
  workspace.dispose();
});

test("published output checks and later consumer failure remain separate, exact and escaped", async () => {
  const { workspace, state, transport } = await setup((path, query) =>
    path.endsWith("/history")
      ? Promise.resolve(json(history(), context(query.branch)))
      : path.endsWith(`/attempts/${publishedAttempt}`)
        ? Promise.resolve(
            json(attempt(publishedAttempt, "output", "PASS"), {
              ...context(query.branch),
              selection: {
                kind: "local_version",
                id: versionId,
                digest: "f".repeat(64),
              },
              fingerprint: "f".repeat(64),
            }),
          )
        : path.endsWith(`/attempts/${failedAttempt}`)
          ? Promise.resolve(
              json(attempt(failedAttempt, "input", "VIOLATION"), planContext()),
            )
          : undefined,
  );
  const view = render(
    <HealthInspector workspace={workspace} state={state} node={node} />,
  );
  await within(
    screen.getByRole("region", { name: "Output dataset checks" }),
  ).findByText("Passed");
  expect(
    within(
      screen.getByRole("region", { name: "Output dataset checks" }),
    ).getByText("Passed"),
  ).toBeTruthy();
  expect(
    within(screen.getByRole("region", { name: "Latest builds" })).getByText(
      "Failed",
    ),
  ).toBeTruthy();
  expect(
    within(
      screen.getByRole("region", { name: "Input dataset checks" }),
    ).getByText("Failed"),
  ).toBeTruthy();
  expect(view.container.querySelectorAll(".health-dot")).toHaveLength(30);
  const buildHistory = screen.getByRole("region", { name: "Latest builds" });
  expect(within(buildHistory).queryAllByRole("button")).toHaveLength(0);
  expect(buildHistory.querySelector(".health-history-details")).toBeNull();
  expect(
    screen.queryByRole("button", { name: "Refresh check evidence" }),
  ).toBeNull();
  expect(
    screen.getByRole("heading", { name: state.value.dataset?.path ?? "" }),
  ).toBeTruthy();
  expect(screen.queryByText("Latest check details")).toBeNull();
  const publishedRegion = screen.getByRole("region", {
    name: "Output dataset checks",
  });
  const publishedCircle = within(publishedRegion).getByRole("button", {
    name: /^1: Passed/,
  });
  expect(publishedCircle.textContent).toBe("");
  expect(publishedCircle.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(publishedCircle);
  expect(publishedCircle.getAttribute("aria-expanded")).toBe("true");
  expect(
    within(publishedRegion).getByText("<img src=x onerror=alert(1)>"),
  ).toBeTruthy();
  expect(
    publishedRegion.querySelector(".health-check .health-dot.health-good"),
  ).toBeTruthy();
  fireEvent.click(
    within(
      screen.getByRole("region", { name: "Input dataset checks" }),
    ).getByRole("button", { name: /^1: Failed/ }),
  );
  expect(
    view.container.querySelector(".health-check .health-dot.health-bad"),
  ).toBeTruthy();
  fireEvent.click(publishedCircle);
  expect(publishedCircle.getAttribute("aria-expanded")).toBe("false");
  expect(
    publishedRegion
      .querySelector(".health-history-details")
      ?.hasAttribute("inert"),
  ).toBe(true);
  expect(view.container.querySelector("img")).toBeNull();
  const publishedRead = transport.mock.calls
    .map(([, init]) => JSON.parse(String(init?.body ?? "{}")))
    .find((body) => body.path?.endsWith(`/attempts/${publishedAttempt}`));
  expect(publishedRead.query.version).toBe(versionId);
  expect(publishedRead.query.branch).toBe("public");
  const recentRead = transport.mock.calls
    .map(([, init]) => JSON.parse(String(init?.body ?? "{}")))
    .find((body) => body.path?.endsWith(`/attempts/${failedAttempt}`));
  expect(recentRead.query.plan).toBe(queryId);
  expect(recentRead.query.branch).toBe("master");
  workspace.dispose();
});
test("a late previous dataset health response cannot paint a changed selection", async () => {
  let complete!: (value: Response) => void;
  const pending = new Promise<Response>((resolve) => {
    complete = resolve;
  });
  const { workspace, state } = await setup((path) =>
    path.endsWith("/history")
      ? Promise.resolve(json({ entries: [], next_cursor: null }, context()))
      : path.endsWith(`/attempts/${publishedAttempt}`)
        ? pending
        : undefined,
  );
  const view = render(
    <HealthInspector workspace={workspace} state={state} node={node} />,
  );
  const changed: Ready = { ...state, value: { ...state.value, dataset: null } };
  view.rerender(
    <HealthInspector workspace={workspace} state={changed} node={undefined} />,
  );
  await act(async () =>
    complete(
      json(attempt(publishedAttempt, "output", "PASS"), {
        ...context(),
        selection: {
          kind: "local_version",
          id: versionId,
          digest: "f".repeat(64),
        },
        fingerprint: "f".repeat(64),
      }),
    ),
  );
  expect(screen.queryByText("PASS")).toBeNull();
  expect(
    screen.getByText("Select a dataset to inspect its health."),
  ).toBeTruthy();
  workspace.dispose();
});

test("foreign health shows retained provider certificates without starting provider reads", async () => {
  const { workspace, state, transport } = await setup();
  if (!state.value.dataset) throw new Error("Missing fixture dataset");
  const foreign: Ready = {
    ...state,
    value: {
      ...state.value,
      dataset: {
        ...state.value.dataset,
        origin: "external",
        workspace_id: queryId,
      },
      versions: {
        entries: [
          {
            version: versionId,
            origin_workspace: queryId,
            dataset: datasetId,
            availability: "not_verified",
            origin: "external",
            metadata: {
              output_certificates: [
                { fingerprint: "f".repeat(64), outcome: "PASS" },
              ],
            },
          },
        ],
        next_cursor: null,
      },
    },
  };
  const before = transport.mock.calls.length;
  render(
    <HealthInspector workspace={workspace} state={foreign} node={undefined} />,
  );
  fireEvent.click(
    within(
      screen.getByRole("region", { name: "Output dataset checks" }),
    ).getByRole("button", { name: /^1: Passed/ }),
  );
  expect(screen.getByText("Certificate 1")).toBeTruthy();
  expect(
    within(
      screen.getByRole("region", { name: "Output dataset checks" }),
    ).getAllByText("Passed").length,
  ).toBeGreaterThan(0);
  expect(transport.mock.calls).toHaveLength(before);
  workspace.dispose();
});

test("health history keeps newest first, ten slots, severity and retained output on input read failure", async () => {
  const base = history().entries[0];
  if (!base) throw new Error("Missing history fixture");
  const jobs = Array.from({ length: 12 }, (_, i) => ({
    ...base,
    id: `00000000-0000-4000-8000-${String(i + 1).padStart(12, "0")}`,
    state: i === 0 ? "SUCCEEDED" : "FAILED",
    created_us: String(1700000000000000 - i * 1000000),
    attempts:
      i === 0 ? [{ ...base.attempts[0], started_us: "1700000000000000" }] : [],
  }));
  const { workspace, state } = await setup((path, query) =>
    path.endsWith("/history")
      ? Promise.resolve(
          json({ entries: jobs, next_cursor: null }, context(query.branch)),
        )
      : path.endsWith(`/attempts/${publishedAttempt}`)
        ? Promise.resolve(
            json(attempt(publishedAttempt, "output", "PASS"), {
              ...context(query.branch),
              selection: {
                kind: "local_version",
                id: versionId,
                digest: "f".repeat(64),
              },
              fingerprint: "f".repeat(64),
            }),
          )
        : path.endsWith(`/attempts/${failedAttempt}`)
          ? Promise.reject(new Error("Retained attempt unavailable"))
          : undefined,
  );
  render(<HealthInspector workspace={workspace} state={state} node={node} />);
  const published = screen.getByRole("region", {
    name: "Output dataset checks",
  });
  await within(published).findByText("Passed");
  expect(screen.getByRole("alert").textContent).toContain(
    "coordinator is disconnected",
  );
  const builds = within(
    screen.getByRole("region", { name: "Latest builds" }),
  ).getAllByRole("listitem");
  expect(builds).toHaveLength(10);
  expect(builds[0]?.getAttribute("aria-label")).toContain("Succeeded");
  expect(builds[1]?.getAttribute("aria-label")).toContain("Failed");
  expect(
    within(
      screen.getByRole("region", { name: "Input dataset checks" }),
    ).getByText("Unavailable"),
  ).toBeTruthy();
  workspace.dispose();
});

test("check circles open their retained evidence and runtime updates close the old selection", async () => {
  const base = history().entries[0];
  if (!base) throw new Error("Missing history fixture");
  const { workspace, state } = await setup((path, query) => {
    if (path.endsWith("/history"))
      return Promise.resolve(
        json(
          {
            entries: [
              base,
              {
                ...base,
                id: publishedAttempt,
                state: "SUCCEEDED",
                created_us: "1699999999000000",
                attempts: [
                  {
                    ...base.attempts[0],
                    attempt: publishedAttempt,
                    state: "SUCCEEDED",
                    started_us: "1699999999000000",
                  },
                ],
              },
            ],
            next_cursor: null,
          },
          context(query.branch),
        ),
      );
    if (path.includes("/attempts/")) {
      const older = path.endsWith(publishedAttempt);
      const value = attempt(
        older ? publishedAttempt : failedAttempt,
        query.version ? "output" : "input",
        older ? "PASS" : "VIOLATION",
      );
      const check = value.evidence.checks[0];
      if (!check) throw new Error("Missing check fixture");
      check.name = older ? "Older passing check" : "Latest required check";
      return Promise.resolve(
        json(
          value,
          query.version
            ? {
                ...context(query.branch),
                selection: {
                  kind: "local_version",
                  id: versionId,
                  digest: "f".repeat(64),
                },
                fingerprint: "f".repeat(64),
              }
            : planContext(),
        ),
      );
    }
    return undefined;
  });
  const view = render(
    <HealthInspector workspace={workspace} state={state} node={node} />,
  );
  const builds = screen.getByRole("region", { name: "Input dataset checks" });
  await within(builds).findByText("Failed");
  expect(within(builds).queryByText("Latest required check")).toBeNull();
  const first = within(builds).getByRole("button", { name: /^1: Failed/ });
  const second = within(builds).getByRole("button", { name: /^2: Passed/ });
  fireEvent.click(first);
  expect(within(builds).getByText("Latest required check")).toBeTruthy();
  fireEvent.click(second);
  expect(within(builds).getByText("Older passing check")).toBeTruthy();
  expect(within(builds).queryByText("Latest required check")).toBeNull();
  expect(first.getAttribute("aria-expanded")).toBe("false");
  expect(second.getAttribute("aria-expanded")).toBe("true");
  expect(
    within(builds).getByRole("button", { name: /^3: No recorded history/ }),
  ).toHaveProperty("disabled", true);
  expect(
    screen.queryByRole("button", { name: "Refresh check evidence" }),
  ).toBeNull();
  view.rerender(
    <HealthInspector
      workspace={workspace}
      state={{
        ...state,
        value: {
          ...state.value,
          context: { ...state.value.context, runtime_revision: "2" },
        },
      }}
      node={node}
    />,
  );
  await waitFor(() =>
    expect(screen.queryByText("Older passing check")).toBeNull(),
  );
  workspace.dispose();
});

test("tooltip separates the latest failed attempt from the retained successful publication", async () => {
  const { workspace } = await setup();
  render(<DatasetTooltip node={node} workspace={workspace} />);
  const tooltip = await screen.findByRole("tooltip");
  const latest = within(tooltip).getByText("Latest attempt");
  expect(latest.nextElementSibling?.textContent).toBe("Failed");
  expect(
    within(tooltip).getByText(/The failed attempt did not publish/),
  ).toBeTruthy();
  expect(tooltip.textContent).not.toContain("Failed · Published");
  workspace.dispose();
});

test("scratchpad pills and filters use submitted SQL, retain loading rows and reset with new runs", async () => {
  const requests: { sql: string; parameters: unknown[] }[] = [];
  let release: ((response: Response) => void) | undefined;
  let hold = false;
  const { workspace, state } = await setup(
    (path) => {
      if (path.endsWith("/results") && hold)
        return new Promise<Response>((resolve) => {
          release = resolve;
        });
      return successful(path);
    },
    (url, init) => {
      if (!url.startsWith("/api/v1/queries?")) return;
      const request = JSON.parse(String(init?.body));
      requests.push(request);
      return Promise.resolve(json({ ...accepted, ...request }, context()));
    },
  );
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByText("9007199254740993");
  fireEvent.change(screen.getByLabelText("SQL query"), {
    target: { value: "SELECT 42 FROM dataset" },
  });
  hold = true;
  fireEvent.click(
    screen.getByRole("button", { name: "Column options for amount" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Sort descending" }));
  await screen.findByRole("status", { name: "Updating preview" });
  await waitFor(() => expect(release).toBeTypeOf("function"));
  expect(screen.getByLabelText("amount: Sort descending")).toBeTruthy();
  expect(requests[1]?.sql).toContain('SELECT * FROM "dataset"\n)');
  expect(requests[1]?.sql).toContain('ORDER BY "amount" DESC');
  await act(async () =>
    release?.(
      json(
        {
          id: queryId,
          schema,
          rows: [[{ type: "i64", value: "9" }]],
          next_offset: null,
          truncated: false,
        },
        context(),
      ),
    ),
  );
  await screen.findByText("9");
  hold = false;
  fireEvent.click(screen.getByRole("button", { name: "Reset preview" }));
  await screen.findByText("9007199254740993");
  expect(requests[2]?.sql).toBe('SELECT * FROM "dataset"');
  expect(screen.queryByLabelText("amount: Sort descending")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await waitFor(() => expect(requests[3]?.sql).toBe("SELECT 42 FROM dataset"));
});

test("scratchpad scroll paging coalesces events and stops after a failed page", async () => {
  let reads = 0;
  let fail: ((reason: Error) => void) | undefined;
  const { workspace, state } = await setup(
    (path, query) => {
      if (path.endsWith("/results")) {
        if (query.offset === "1") {
          reads++;
          return new Promise<Response>((_, reject) => {
            fail = reject;
          });
        }
        expect(query.limit).toBe("500");
        return Promise.resolve(
          json(
            {
              id: queryId,
              schema,
              rows: [[{ type: "i64", value: "17" }]],
              next_offset: 1,
              truncated: false,
            },
            context(),
          ),
        );
      }
      return successful(path);
    },
    (url) =>
      url.startsWith("/api/v1/queries?")
        ? Promise.resolve(json(accepted, context()))
        : undefined,
  );
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByText("17");
  const grid = screen.getByLabelText("SQL query results preview");
  fireEvent.wheel(grid, { deltaY: 100 });
  fireEvent.wheel(grid, { deltaY: 100 });
  await waitFor(() => expect(reads).toBe(1));
  await act(async () => fail?.(new Error("network offline")));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "The coordinator is disconnected.",
  );
  await act(async () => {
    fireEvent.wheel(grid, { deltaY: 100 });
  });
  expect(reads).toBe(1);
  expect(screen.getByText("17")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "More result rows" })).toBeNull();
});

test("scratchpad autocompletes catalogue paths with keyboard selection and keeps joined bindings through sorting", async () => {
  let currentVersion = versionId;
  let lookups = 0;
  const requests: { sql: string; bindings: ApiQueryV1["bindings"] }[] = [];
  const { workspace, state } = await setup(
    (path, query) => {
      if (
        path === "/api/v1/datasets" &&
        (query.fuzzy !== undefined || query.filter)
      ) {
        if (query.filter) lookups++;
        return Promise.resolve(
          json(
            {
              entries: [
                {
                  ...dataset,
                  path: "curated/east",
                  head: { version: currentVersion, resolved_branch: "public" },
                },
              ],
              total: "1",
              next_cursor: null,
              schema: null,
              freshness: "unknown",
            },
            context(),
          ),
        );
      }
      return successful(path);
    },
    (url, init) => {
      if (!url.startsWith("/api/v1/queries?")) return;
      const request = JSON.parse(String(init?.body));
      requests.push(request);
      return Promise.resolve(json({ ...accepted, ...request }, context()));
    },
  );
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  const editor = screen.getByRole("textbox", { name: "SQL query" });
  fireEvent.change(editor, {
    target: { value: "SELECT * FROM dataset JOIN cu" },
  });
  const option = await screen.findByRole("option", {
    name: "curated/east, Dataset",
  });
  expect(option.getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(editor, { key: "Enter" });
  expect((editor as HTMLTextAreaElement).value).toBe(
    'SELECT * FROM dataset JOIN "curated/east" ',
  );
  expect(screen.queryByRole("listbox")).toBeNull();
  fireEvent.change(editor, {
    target: {
      value:
        'SELECT d.amount FROM dataset d JOIN "curated/east" e ON d.amount = e.amount',
    },
  });
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByText("9007199254740993");
  expect(requests[0]?.sql).toContain('JOIN "__tf_catalogue_1" e');
  expect(requests[0]?.bindings).toHaveLength(2);
  expect(
    screen.getByText(/Using published data: curated\/east from branch public/),
  ).toBeTruthy();
  currentVersion = queryId;
  fireEvent.click(
    screen.getByRole("button", { name: "Column options for amount" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Sort descending" }));
  await waitFor(() => expect(requests).toHaveLength(2));
  expect(requests[1]?.bindings).toEqual(requests[0]?.bindings);
  expect(lookups).toBe(1);
  await screen.findByRole("button", { name: "Download TSV" });
  await waitFor(() =>
    expect(
      screen
        .getByRole("button", { name: "Run query" })
        .hasAttribute("disabled"),
    ).toBe(false),
  );
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await waitFor(() => expect(requests).toHaveLength(3));
  expect(requests[2]?.bindings[1]?.version).toBe(queryId);
  expect(requests[2]?.bindings[0]?.version).toBe(versionId);
  expect(lookups).toBe(2);
  workspace.dispose();
});

test("scratchpad starts with and completes the selected quoted catalogue name", async () => {
  const { workspace, state } = await setup((path) => {
    if (path === "/api/v1/datasets")
      return Promise.resolve(
        json(
          {
            entries: [dataset],
            total: "1",
            next_cursor: null,
            schema: null,
            freshness: "unknown",
          },
          context(),
        ),
      );
    return successful(path);
  });
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  const editor = screen.getByRole("textbox", { name: "SQL query" });
  expect((editor as HTMLTextAreaElement).value).toBe(
    'SELECT * FROM "raw/example"',
  );
  fireEvent.change(editor, { target: { value: 'SELECT * FROM "raw/' } });
  const selected = await screen.findByRole("option", {
    name: "raw/example, Selected dataset",
  });
  expect(screen.getAllByRole("option")).toHaveLength(1);
  fireEvent.click(selected);
  expect((editor as HTMLTextAreaElement).value).toBe(
    'SELECT * FROM "raw/example" ',
  );
  workspace.dispose();
});

test("scratchpad catalogue failures do not accept a query", async () => {
  const { workspace, state, transport } = await setup((path, query) => {
    if (path === "/api/v1/datasets" && query.filter)
      return Promise.resolve(
        json(
          {
            entries: [],
            total: "0",
            next_cursor: null,
            schema: null,
            freshness: "unknown",
          },
          context(),
        ),
      );
  });
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.change(screen.getByLabelText("SQL query"), {
    target: { value: 'SELECT * FROM "missing/dataset"' },
  });
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "not found in this catalogue",
  );
  expect(
    transport.mock.calls.some(([url]) =>
      String(url).startsWith("/api/v1/queries?"),
    ),
  ).toBe(false);
  workspace.dispose();
});

test("catalogue completion ignores old searches and supports escape, arrow keys and tab", async () => {
  let release: ((response: Response) => void) | undefined;
  const { workspace, state } = await setup((path, query) => {
    if (path !== "/api/v1/datasets" || query.fuzzy === undefined) return;
    if (query.fuzzy === "old")
      return new Promise<Response>((resolve) => {
        release = resolve;
      });
    return Promise.resolve(
      json(
        {
          entries: [
            { ...dataset, path: "curated/east" },
            { ...dataset, path: "curated/west" },
          ],
          total: "2",
          next_cursor: null,
          schema: null,
          freshness: "unknown",
        },
        context(),
      ),
    );
  });
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  const editor = screen.getByLabelText("SQL query");
  fireEvent.change(editor, { target: { value: "SELECT * FROM old" } });
  await waitFor(() => expect(release).toBeTypeOf("function"));
  fireEvent.change(editor, { target: { value: "SELECT * FROM cu" } });
  await screen.findByRole("option", { name: "curated/east, Dataset" });
  await act(async () =>
    release?.(
      json(
        {
          entries: [{ ...dataset, path: "old/result" }],
          total: "1",
          next_cursor: null,
          schema: null,
          freshness: "unknown",
        },
        context(),
      ),
    ),
  );
  expect(screen.queryByRole("option", { name: /old\/result/ })).toBeNull();
  fireEvent.keyDown(editor, { key: "ArrowDown" });
  expect(
    screen
      .getByRole("option", { name: "curated/west, Dataset" })
      .getAttribute("aria-selected"),
  ).toBe("true");
  fireEvent.keyDown(editor, { key: "Tab" });
  expect((editor as HTMLTextAreaElement).value).toBe(
    'SELECT * FROM "curated/west" ',
  );
  fireEvent.change(editor, { target: { value: "SELECT * FROM cu" } });
  await screen.findByRole("listbox");
  fireEvent.keyDown(editor, { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
  workspace.dispose();
});
