import { expect, test, vi } from "vitest";
import { ApiFailure, Client } from "../api/client";
import { Workspace } from "../workspace";
import { context, fixtureTransport, json } from "../test-fixtures";
import type { ApiContextV1, ApiLineageNodeV1 } from "../generated/contracts";
import { GraphModel } from "./model";
import { GraphCache, type Scope } from "./cache";

const node = (
  id: string,
  latestAttempt: NonNullable<
    ApiLineageNodeV1["overlay"]
  >["latest_attempt"] = "Succeeded",
): ApiLineageNodeV1 => ({
  identity: id,
  paths: [id],
  depth: "0",
  external: false,
  producer: true,
  parent_count: "0",
  child_count: "0",
  resource_type: "polars_transform",
  publication: "published",
  overlay: {
    version: null,
    rows: "2",
    files: "1",
    bytes: "100",
    latest_attempt: latestAttempt,
    freshness: "unknown",
    health: "NoChecks",
    input_failed: false,
    reasons: [],
    durations: null,
  },
});
async function fixture() {
  let revision = 1;
  let changedContext: Partial<ApiContextV1> = {};
  const base = fixtureTransport();
  const transport: typeof fetch = async (url, init) => {
    const response = await base(url, init);
    if (revision === 1) return response;
    const value = (await response.json()) as {
      data: unknown;
      context: unknown;
    };
    const c = {
      ...context(),
      ...changedContext,
      runtime_revision: String(revision),
      fingerprint: String(revision).repeat(64),
    };
    const request = JSON.parse(String(init?.body)) as { path?: string };
    return json(
      request.path === "/api/v1/context" ? c : value.data,
      value.context === null ? null : c,
    );
  };
  const workspace = new Workspace(new Client(transport));
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const disconnect = model.connect();
  model.restore({ nodes: [node("root")], edges: [] });
  return {
    model,
    change: async (patch: Partial<ApiContextV1> = {}) => {
      revision++;
      changedContext = patch;
      await workspace.refresh(true);
    },
    dispose: () => {
      disconnect();
      workspace.dispose();
    },
  };
}
test("transient metadata conflicts recover instead of leaving successful nodes Unknown", async () => {
  const f = await fixture();
  const read = vi.spyOn(GraphCache.prototype, "lookup");
  read.mockRejectedValueOnce(new ApiFailure("conflict", "Changed during read"));
  read.mockResolvedValue({ nodes: [node("root")], edges: [] });
  try {
    await f.change();
    expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
      "Succeeded",
    );
    await vi.waitFor(
      () =>
        expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
          "Succeeded",
        ),
      { timeout: 2000 },
    );
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2), {
      timeout: 2000,
    });
    expect(f.model.snapshot().message).toBe("");
  } finally {
    f.dispose();
    read.mockRestore();
  }
});
test("runtime updates keep confirmed states until the replacement arrives, even across superseded reads", async () => {
  const f = await fixture();
  const replies: Array<(scope: Scope) => void> = [];
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockImplementation(() => new Promise((resolve) => replies.push(resolve)));
  const states: Array<string | null | undefined> = [];
  const unsubscribe = f.model.subscribe(() =>
    states.push(f.model.snapshot().nodes[0]?.overlay?.latest_attempt),
  );
  const building = node("root", "Running");
  try {
    await f.change();
    replies[0]?.({ nodes: [building], edges: [] });
    await vi.waitFor(() =>
      expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
        "Running",
      ),
    );
    const previous = f.model.snapshot().nodes;
    await f.change();
    expect(f.model.snapshot().nodes).toBe(previous);
    await f.change();
    expect(f.model.snapshot().nodes).toBe(previous);
    replies[2]?.({ nodes: [node("root")], edges: [] });
    await vi.waitFor(() =>
      expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
        "Succeeded",
      ),
    );
    replies[1]?.({ nodes: [building], edges: [] });
    await Promise.resolve();
    expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
      "Succeeded",
    );
    expect(states).toContain("Running");
    expect(
      states.every((state) => state === "Running" || state === "Succeeded"),
    ).toBe(true);
  } finally {
    unsubscribe();
    f.dispose();
    read.mockRestore();
  }
});
test.each([
  { source: "dddddddd-dddd-4ddd-8ddd-dddddddddddd" },
  { graph: "f".repeat(64) },
  { registry: "f".repeat(64) },
  { configuration: "f".repeat(64) },
])(
  "same-selector capture changes retain the verified snapshot while loading: %j",
  async (patch) => {
    const f = await fixture();
    const read = vi
      .spyOn(GraphCache.prototype, "lookup")
      .mockImplementation(() => new Promise(() => {}));
    try {
      await f.change(patch);
      expect(f.model.snapshot().nodes.map((n) => n.identity)).toEqual(["root"]);
      expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
        "Succeeded",
      );
      expect(f.model.snapshot().nodes[0]?.publication).toBe("published");
    } finally {
      f.dispose();
      read.mockRestore();
    }
  },
);
test("fallback policy changes clear the previous branch resolution while loading", async () => {
  const f = await fixture();
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockImplementation(() => new Promise(() => {}));
  try {
    await f.change({ fallback_policy: ["master"] });
    expect(f.model.snapshot().nodes.map((n) => n.identity)).toEqual(["root"]);
    expect(f.model.snapshot().nodes[0]?.overlay).toBeUndefined();
    expect(f.model.snapshot().nodes[0]?.publication).toBe("unknown");
  } finally {
    f.dispose();
    read.mockRestore();
  }
});
test("a verified unavailable status replaces the previous success", async () => {
  const f = await fixture();
  const unavailable = { ...node("root") };
  delete unavailable.overlay;
  unavailable.publication = "unknown";
  const read = vi.spyOn(GraphCache.prototype, "lookup").mockResolvedValue({
    nodes: [unavailable],
    edges: [],
  });
  try {
    await f.change();
    await vi.waitFor(() =>
      expect(f.model.snapshot().nodes[0]?.overlay).toBeUndefined(),
    );
    expect(f.model.snapshot().nodes[0]?.publication).toBe("unknown");
  } finally {
    f.dispose();
    read.mockRestore();
  }
});
test("concurrent graph adds/removes do not cancel refresh or restore removed nodes", async () => {
  const f = await fixture();
  let release: ((scope: Scope) => void) | undefined;
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockImplementation((ids) =>
      ids.includes("root")
        ? new Promise((resolve) => {
            release = resolve;
          })
        : Promise.resolve({ nodes: [node("extra")], edges: [] }),
    );
  try {
    await f.change();
    await f.model.add("extra");
    f.model.remove(new Set(["root"]));
    release?.({ nodes: [node("root")], edges: [] });
    await Promise.resolve();
    expect(f.model.snapshot().nodes.map((n) => n.identity)).toEqual(["extra"]);
    expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
      "Succeeded",
    );
    expect(f.model.snapshot().message).toBe("");
  } finally {
    f.dispose();
    read.mockRestore();
  }
});
test("deterministic metadata failures stay visible without retrying", async () => {
  const f = await fixture();
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockRejectedValue(new ApiFailure("failed", "Unavailable metadata"));
  try {
    await f.change();
    await vi.waitFor(() =>
      expect(f.model.snapshot().message).toBe("Unavailable metadata"),
    );
    expect(read).toHaveBeenCalledOnce();
    expect(f.model.snapshot().nodes[0]?.overlay).toBeUndefined();
  } finally {
    f.dispose();
    read.mockRestore();
  }
});
test("repeated conflicts stop after three tries", async () => {
  const f = await fixture();
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockRejectedValue(new ApiFailure("conflict", "Still changing"));
  vi.useFakeTimers();
  try {
    await f.change();
    await vi.advanceTimersByTimeAsync(1500);
    expect(read).toHaveBeenCalledTimes(3);
    expect(f.model.snapshot().message).toBe("Still changing");
    expect(f.model.snapshot().nodes[0]?.overlay).toBeUndefined();
  } finally {
    f.dispose();
    read.mockRestore();
    vi.useRealTimers();
  }
});

test("disconnect cancels a pending metadata retry", async () => {
  const f = await fixture();
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockRejectedValue(new ApiFailure("conflict", "Changing"));
  vi.useFakeTimers();
  try {
    await f.change();
    expect(read).toHaveBeenCalledOnce();
    f.dispose();
    await vi.advanceTimersByTimeAsync(2000);
    expect(read).toHaveBeenCalledOnce();
    expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
      "Succeeded",
    );
  } finally {
    f.dispose();
    read.mockRestore();
    vi.useRealTimers();
  }
});

test("branch switches preserve membership, clear old facts and discard late branch replies", async () => {
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const model = new GraphModel(workspace);
  const disconnect = model.connect();
  model.restore({ nodes: [node("root")], edges: [] });
  const visual = model.snapshot().visualContext;
  let release: ((scope: Scope) => void) | undefined;
  const read = vi
    .spyOn(GraphCache.prototype, "lookup")
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    )
    .mockResolvedValue({
      nodes: [{ ...node("root"), publication: "missing" }],
      edges: [],
    });
  try {
    await workspace.select({ branch: "review" });
    expect(model.snapshot().visualContext).toBe(visual);
    expect(model.snapshot().nodes[0]?.overlay).toBeUndefined();
    await workspace.select({ branch: "master" });
    await vi.waitFor(() =>
      expect(model.snapshot().nodes[0]?.publication).toBe("missing"),
    );
    release?.({ nodes: [node("root")], edges: [] });
    await Promise.resolve();
    expect(model.snapshot().nodes[0]?.publication).toBe("missing");
    expect(model.snapshot().nodes.map((n) => n.identity)).toEqual(["root"]);
  } finally {
    disconnect();
    workspace.dispose();
    read.mockRestore();
  }
});
