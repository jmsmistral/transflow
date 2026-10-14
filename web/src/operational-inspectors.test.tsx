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
  sql: "SELECT * FROM dataset",
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
test("scratchpad shows editor and empty results before a query, with contextual parameters help", async () => {
  const { workspace, state } = await setup();
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  const editor = screen.getByRole("region", { name: "SQL editor" });
  const results = screen.getByRole("region", { name: "SQL results" });
  expect(within(editor).getByLabelText("SQL query")).toBeTruthy();
  expect(
    within(results).getByRole("heading", { name: "Results" }),
  ).toBeTruthy();
  expect(
    within(results).getByText(
      "Run a SQL query in the editor to see a preview of the results.",
    ),
  ).toBeTruthy();
  expect(screen.queryByText("SQL scratchpad")).toBeNull();
  expect(
    screen.queryByText("Query the selected published version as dataset."),
  ).toBeNull();
  const disclosure = editor.querySelector("details");
  expect(disclosure?.open).toBe(false);
  fireEvent.click(within(editor).getByText("Parameters"));
  expect(within(editor).getByText("WHERE id = CAST(? AS BIGINT)")).toBeTruthy();
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
  fireEvent.click(
    await screen.findByRole("button", { name: "More result rows" }),
  );
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

test("invalid bound parameters do not start a query", async () => {
  const { workspace, state, transport } = await setup();
  render(
    <ScratchpadInspector workspace={workspace} state={state} dark={false} />,
  );
  fireEvent.change(screen.getByLabelText("Bound values (JSON array)"), {
    target: { value: "[42]" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Run query" }));
  await screen.findByRole("alert");
  expect(
    transport.mock.calls.some(([url]) =>
      String(url).startsWith("/api/v1/queries?"),
    ),
  ).toBe(false);
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
test("published output checks and later consumer failure remain separate, exact and escaped", async () => {
  const { workspace, state, transport } = await setup((path, query) =>
    path.endsWith("/history")
      ? Promise.resolve(json(history(), context(query.branch)))
      : path.endsWith(`/attempts/${publishedAttempt}`)
        ? Promise.resolve(
            json(attempt(publishedAttempt, "output", "PASS"), {
              ...context(),
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
  await screen.findByText("PASS");
  await screen.findByText("VIOLATION");
  expect(
    screen.getByText("Published output checks").nextSibling?.textContent,
  ).toBe("Passed");
  expect(
    screen.getByText("Latest attempt on master").nextSibling?.textContent,
  ).toBe("Failed");
  expect(screen.getByText(/latest consumer input check failed/)).toBeTruthy();
  expect(view.container.querySelector("img")).toBeNull();
  const publishedRead = transport.mock.calls
    .map(([, init]) => JSON.parse(String(init?.body ?? "{}")))
    .find((body) => body.path?.endsWith(`/attempts/${publishedAttempt}`));
  expect(publishedRead.query.version).toBe(versionId);
  const recentRead = transport.mock.calls
    .map(([, init]) => JSON.parse(String(init?.body ?? "{}")))
    .find((body) => body.path?.endsWith(`/attempts/${failedAttempt}`));
  expect(recentRead.query.plan).toBe(queryId);
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
  expect(screen.getByText("Provider output certificates")).toBeTruthy();
  expect(screen.getByText("PASS")).toBeTruthy();
  expect(
    screen.getByText("Published output checks").nextSibling?.textContent,
  ).toBe("Unavailable");
  expect(transport.mock.calls).toHaveLength(before);
  workspace.dispose();
});
