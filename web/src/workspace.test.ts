import { expect, test, vi } from "vitest";
import { Client, decode } from "./api/client";
import { Workspace } from "./workspace";
import {
  workspaceId,
  datasetId,
  versionId,
  context,
  dataset,
  capabilities,
  json,
  fixtureTransport,
} from "./test-fixtures";

function deferred<T>() {
  let resolve: (value: T) => void = () => {
    throw new Error("not initialized");
  };
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

test("abort-ignoring old branch responses never replace a newer snapshot", async () => {
  const late = deferred<Response>();
  let hold = false;
  let oldSignal: AbortSignal | null | undefined;
  const model = new Workspace(
    new Client(
      fixtureTransport((path, q, signal) => {
        if (hold && path.endsWith("/datasets") && q.branch === "master") {
          oldSignal = signal;
          return late.promise;
        }
        return undefined;
      }),
    ),
  );
  await model.connect("launch");
  hold = true;
  const first = model.select({ branch: "master" });
  await vi.waitFor(() => expect(oldSignal).toBeTruthy());
  const second = model.select({ branch: "feature" });
  expect(model.snapshot().kind).toBe("loading");
  expect(oldSignal?.aborted).toBe(true);
  await second;
  late.resolve(
    json(
      {
        entries: [{ ...dataset, path: "old/rows_and_code" }],
        total: "1",
        next_cursor: null,
        schema: null,
        freshness: "unknown",
      },
      context(),
    ),
  );
  await first;
  const state = model.snapshot();
  expect(state.kind).toBe("ready");
  if (state.kind === "ready") {
    expect(state.value.context.branch).toBe("feature");
    expect(state.value.datasets.entries[0]?.path).toBe("raw/example");
  }
  model.dispose();
});
test("wrong fingerprint or workspace fails closed, with no previous dataset content", async () => {
  for (const mismatch of [
    { fingerprint: "f".repeat(64) },
    { workspace: datasetId },
  ]) {
    const model = new Workspace(
      new Client(
        fixtureTransport((path) =>
          path.endsWith("/datasets")
            ? Promise.resolve(
                json(
                  {
                    entries: [dataset],
                    total: "1",
                    next_cursor: null,
                    schema: null,
                    freshness: "unknown",
                  },
                  { ...context(), ...mismatch },
                ),
              )
            : undefined,
        ),
      ),
    );
    await model.connect("launch");
    expect(model.snapshot().kind).toBe("failed");
    expect("value" in model.snapshot()).toBe(false);
    model.dispose();
  }
});
test("selected versions carry dataset and provider identity on every dependent read", async () => {
  const transport = fixtureTransport();
  const model = new Workspace(new Client(transport));
  await model.connect("launch");
  await model.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
    version: versionId,
  });
  const state = model.snapshot();
  expect(state.kind).toBe("ready");
  if (state.kind === "ready")
    expect(state.value.context.selection.id).toBe(versionId);
  const calls = vi
    .mocked(transport)
    .mock.calls.map(
      ([, init]) =>
        JSON.parse(String(init?.body)) as { query?: Record<string, string> },
    );
  const pinned = calls.filter((call) => call.query?.version === versionId);
  expect(pinned).toHaveLength(6);
  for (const call of pinned)
    expect(call.query).toMatchObject({
      dataset: datasetId,
      origin_workspace: workspaceId,
      branch: "master",
    });
  model.dispose();
});
test("unknown capabilities, expired session and network failure remain explicit", async () => {
  for (const transport of [
    fixtureTransport((p) =>
      p.endsWith("/capabilities")
        ? Promise.resolve(json({ ...capabilities, api_version: 2 }))
        : undefined,
    ),
    vi.fn<typeof fetch>(async () => new Response("", { status: 401 })),
    vi.fn<typeof fetch>(async () => {
      throw new TypeError("offline");
    }),
  ]) {
    const model = new Workspace(new Client(transport));
    await model.connect("launch");
    expect(["failed", "disconnected"]).toContain(model.snapshot().kind);
    model.dispose();
  }
});
test("source and preview reads reject context mismatch at their shared boundary", async () => {
  const client = new Client(
    vi.fn(async () =>
      json({ text: "old source", rows: ["old rows"] }, context("feature")),
    ),
  );
  for (const schema of ["ApiSourceV1", "ApiPreviewV1"] as const)
    await expect(
      client.read(
        schema,
        "/api/v1/source/example",
        { branch: "master" },
        new AbortController().signal,
        context(),
      ),
    ).rejects.toMatchObject({ kind: "conflict" });
});
test("schema validation rejects unknown fields and renders no unvalidated capability data", () => {
  expect(() =>
    decode("ApiCapabilitiesV1", {
      ...capabilities,
      limits: { ...capabilities.limits, page_max: -1 },
    }),
  ).toThrow();
});

test("event resync racing a user selection waits for the latest read models", async () => {
  const late = deferred<Response>();
  let hold = false;
  let blocked = false;
  let stream: ReadableStreamDefaultController<Uint8Array> | undefined;
  const model = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        if (path.endsWith("/events"))
          return Promise.resolve(
            new Response(
              new ReadableStream<Uint8Array>({
                start(controller) {
                  stream = controller;
                },
              }),
              { headers: { "content-type": "text/event-stream" } },
            ),
          );
        if (hold && path.endsWith("/datasets") && q.branch === "master") {
          blocked = true;
          return late.promise;
        }
        return undefined;
      }),
    ),
  );
  await model.connect("launch");
  await vi.waitFor(() => expect(stream).toBeTruthy());
  hold = true;
  stream?.enqueue(
    new TextEncoder().encode(
      `event: resync_required\nid: 42\ndata: ${JSON.stringify({ workspace: workspaceId, cursor: "42" })}\n\n`,
    ),
  );
  await vi.waitFor(() => expect(blocked).toBe(true));
  await model.select({ branch: "feature" });
  late.resolve(
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
  await vi.waitFor(() => expect(model.snapshot().kind).toBe("ready"));
  expect(model.snapshot().selection.branch).toBe("feature");
  model.dispose();
});

test("a late source inspector response is discarded after selection changes", async () => {
  const late = deferred<Response>();
  const model = new Workspace(
    new Client(
      fixtureTransport((path) =>
        path.includes("/source/") ? late.promise : undefined,
      ),
    ),
  );
  await model.connect("launch");
  const read = model.read("ApiSourceV1", `/api/v1/source/${workspaceId}`, {
    path: "src/example.py",
  });
  const rejected = expect(read).rejects.toMatchObject({ name: "AbortError" });
  await model.select({ branch: "feature" });
  late.resolve(
    json(
      {
        source: workspaceId,
        path: "src/example.py",
        offset: 0,
        text: "old source",
        next_offset: null,
      },
      context(),
    ),
  );
  await rejected;
  expect(model.snapshot().selection.branch).toBe("feature");
  model.dispose();
});

test("branch picker loads all pages and fallback is explicit in contextual reads", async () => {
  const cursors: (string | undefined)[] = [];
  const fallbacks: (string | undefined)[] = [];
  const model = new Workspace(
    new Client(
      fixtureTransport((path, q) => {
        fallbacks.push(q.fallback);
        if (path.endsWith("/branches")) {
          cursors.push(q.cursor);
          return Promise.resolve(
            json(
              {
                entries: [{ name: q.cursor ? "later" : "master" }],
                next_cursor: q.cursor ? null : "next",
              },
              context(q.branch),
            ),
          );
        }
      }),
    ),
  );
  await model.connect("launch");
  const state = model.snapshot();
  expect(state.kind).toBe("ready");
  if (state.kind === "ready")
    expect(state.value.branches.entries).toEqual([
      { name: "master" },
      { name: "later" },
    ]);
  expect(cursors).toEqual([undefined, "next"]);
  await model.select({ branch: "master", fallback: [] });
  expect(fallbacks).toContain("[]");
  model.dispose();
});

test("dataset-only selection preserves ready context while metadata loads", async () => {
  const late = deferred<Response>();
  let hold = false;
  const reads: string[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) => {
        reads.push(path);
        if (hold && path.endsWith(`/datasets/${datasetId}`))
          return late.promise;
      }),
    ),
  );
  await workspace.connect("launch");
  const before = workspace.snapshot();
  reads.length = 0;
  hold = true;
  const pending = workspace.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  expect(workspace.snapshot()).toBe(before);
  late.resolve(json(dataset, context()));
  await pending;
  expect(workspace.snapshot().kind).toBe("ready");
  expect(workspace.snapshot().selection.dataset).toBe(datasetId);
  expect(reads).not.toContain("/api/v1/context");
  expect(reads).not.toContain("/api/v1/datasets");
  hold = false;
  reads.length = 0;
  await workspace.refresh();
  expect(reads).toContain("/api/v1/context");
  expect(reads).toContain("/api/v1/datasets");
  workspace.dispose();
});

test("saved-view opening waits for an event refresh that supersedes its context load", async () => {
  const held = deferred<Response>();
  let hold = false;
  const model = new Workspace(
    new Client(
      fixtureTransport((path) => {
        if (hold && path === "/api/v1/context") return held.promise;
        return undefined;
      }),
    ),
  );
  await model.connect("launch");
  hold = true;
  const pending = model.refresh();
  let settled = false;
  const opening = model.settled().then(() => {
    settled = true;
  });
  await Promise.resolve();
  expect(settled).toBe(false);
  held.resolve(json(context(), context()));
  await pending;
  await opening;
  expect(model.snapshot().kind).toBe("ready");
  expect(settled).toBe(true);
  model.dispose();
});
