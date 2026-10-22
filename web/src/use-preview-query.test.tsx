import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type {
  ApiQueryRequestV1,
  ApiQueryV1,
  ApiQueryResultsV1,
} from "./generated/contracts";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import { usePreviewQuery } from "./use-preview-query";
import {
  context,
  datasetId,
  versionId,
  workspaceId,
  json,
} from "./test-fixtures";

const queryId = "11111111-1111-4111-8111-111111111111";
const c = context();
const request: ApiQueryRequestV1 = {
  sql: 'SELECT * FROM dataset ORDER BY "id" DESC NULLS LAST',
  parameters: [],
  bindings: [
    {
      alias: "dataset",
      dataset: datasetId,
      origin_workspace: workspaceId,
      version: versionId,
    },
  ],
  rows: 1000,
};
const schema: ApiQueryResultsV1["schema"] = {
  format_version: 1,
  fields: [{ name: "id", logical_type: { type: "i64" }, nullable: false }],
};
function job(input: ApiQueryRequestV1): ApiQueryV1 {
  return {
    ...input,
    id: queryId,
    state: "SUCCEEDED",
    elapsed_ms: "1",
    error: null,
    truncated: true,
    rows: "1000",
    limits: {
      rows: 1000,
      bytes: 2097152,
      spill_bytes: "0",
      threads: 1,
      memory_bytes: null,
    },
    timeout_seconds: "30",
    expires_in_seconds: 300,
  };
}
function fixture(
  options: {
    holdAcceptance?: boolean;
    failMore?: boolean;
    wrongPage?: boolean;
    holdPage?: boolean;
  } = {},
) {
  let accepted = job(request);
  let release: (() => void) | undefined;
  let releasePage: (() => void) | undefined;
  const transport: typeof fetch = vi.fn(async (url, init) => {
    if (String(url).startsWith("/api/v1/queries?")) {
      accepted = job(JSON.parse(String(init?.body)) as ApiQueryRequestV1);
      if (options.holdAcceptance)
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      return json(accepted, c);
    }
    if (String(url).includes("/cancel?"))
      return json({ ...accepted, state: "CANCELED" }, c);
    const body = JSON.parse(String(init?.body)) as {
      path: string;
      query: Record<string, string>;
    };
    if (body.path === "/api/v1/context") return json(c, c);
    if (body.path.endsWith("/results")) {
      const offset = Number(body.query.offset);
      if (offset && options.failMore) throw new Error("Result expired");
      if (options.holdPage)
        await new Promise<void>((resolve) => {
          releasePage = resolve;
        });
      return json(
        {
          id: options.wrongPage ? workspaceId : queryId,
          schema,
          rows: Array.from({ length: 500 }, (_, i) => [
            { type: "i64", value: String(1500 - offset - i) },
          ]),
          next_offset: offset === 0 ? 500 : null,
          truncated: true,
        },
        c,
      );
    }
    return json(accepted, c);
  });
  const workspace = new Workspace(new Client(transport));
  return {
    workspace,
    transport: vi.mocked(transport),
    release: () => release?.(),
    releasePage: () => releasePage?.(),
  };
}
function Harness({
  workspace,
  input = request,
}: {
  workspace: Workspace;
  input?: ApiQueryRequestV1 | null;
}) {
  const query = usePreviewQuery(workspace, c, input);
  return (
    <>
      <output>
        {query.error ||
          (query.loading ? "Loading" : `${query.page?.rows.length ?? 0} rows`)}
      </output>
      <span>
        {query.page?.rows[0]?.[0]?.type === "i64"
          ? query.page.rows[0][0].value
          : ""}
      </span>
      <button onClick={query.more}>More</button>
      <button onClick={query.restart}>Restart</button>
      <button onClick={query.cancel}>Cancel</button>
    </>
  );
}
test("sorted results page in 500-row batches from the same query with one request in flight", async () => {
  const f = fixture();
  const view = render(<Harness workspace={f.workspace} />);
  await screen.findByText("500 rows");
  expect(screen.getByText("1500")).toBeTruthy();
  fireEvent.click(screen.getByText("More"));
  fireEvent.click(screen.getByText("More"));
  await screen.findByText("1000 rows");
  const results = f.transport.mock.calls.filter(([, init]) =>
    String(init?.body).includes("/results"),
  );
  expect(results).toHaveLength(2);
  expect(JSON.parse(String(results[1]?.[1]?.body)).query).toMatchObject({
    offset: "500",
    limit: "500",
  });
  view.unmount();
  f.workspace.dispose();
});
test("a failed result read retains prior rows and scroll cannot repeatedly retry", async () => {
  const f = fixture({ failMore: true });
  const view = render(<Harness workspace={f.workspace} />);
  await screen.findByText("500 rows");
  fireEvent.click(screen.getByText("More"));
  await screen.findByText(/The coordinator is disconnected/);
  expect(screen.getByText("1500")).toBeTruthy();
  fireEvent.click(screen.getByText("More"));
  expect(
    f.transport.mock.calls.filter(([, init]) =>
      String(init?.body).includes("/results"),
    ),
  ).toHaveLength(2);
  view.unmount();
  f.workspace.dispose();
});
test("late acceptance is canceled by its owned receipt after the selection closes", async () => {
  const f = fixture({ holdAcceptance: true });
  const view = render(<Harness workspace={f.workspace} />);
  await waitFor(() =>
    expect(
      f.transport.mock.calls.some(([url]) =>
        String(url).startsWith("/api/v1/queries?"),
      ),
    ).toBe(true),
  );
  view.unmount();
  await act(async () => f.release());
  await waitFor(() =>
    expect(
      f.transport.mock.calls.some(([url]) =>
        String(url).includes(`/${queryId}/cancel?`),
      ),
    ).toBe(true),
  );
  f.workspace.dispose();
});
test("query mismatch fails visibly without accepting rows", async () => {
  const f = fixture({ wrongPage: true });
  const view = render(<Harness workspace={f.workspace} />);
  await screen.findByText("The preview page does not match this query.");
  expect(screen.queryByText("1500")).toBeNull();
  view.unmount();
  f.workspace.dispose();
});
test("a late page from a cleared query never appears in the new view", async () => {
  const f = fixture({ holdPage: true });
  const view = render(<Harness workspace={f.workspace} />);
  await waitFor(() =>
    expect(
      f.transport.mock.calls.some(([, init]) =>
        String(init?.body).includes("/results"),
      ),
    ).toBe(true),
  );
  view.rerender(<Harness workspace={f.workspace} input={null} />);
  await act(async () => f.releasePage());
  expect(screen.queryByText("1500")).toBeNull();
  view.unmount();
  f.workspace.dispose();
});
