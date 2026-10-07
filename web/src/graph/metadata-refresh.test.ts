import { expect, test, vi } from "vitest";
import { ApiFailure, Client } from "../api/client";
import { Workspace } from "../workspace";
import { context, fixtureTransport, json } from "../test-fixtures";
import type { ApiLineageNodeV1 } from "../generated/contracts";
import { GraphModel } from "./model";
import { GraphCache, type Scope } from "./cache";

const node = (id: string): ApiLineageNodeV1 => ({
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
    latest_attempt: "Succeeded",
    freshness: "unknown",
    health: "NoChecks",
    input_failed: false,
    reasons: [],
    durations: null,
  },
});
async function fixture() {
  let revision = false;
  const base = fixtureTransport();
  const transport: typeof fetch = async (url, init) => {
    const response = await base(url, init);
    if (!revision) return response;
    const value = (await response.json()) as {
      data: unknown;
      context: unknown;
    };
    const c = {
      ...context(),
      runtime_revision: "2",
      fingerprint: "f".repeat(64),
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
    change: async () => {
      revision = true;
      await workspace.refresh();
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
    expect(f.model.snapshot().nodes[0]?.overlay).toBeUndefined();
    await vi.waitFor(
      () =>
        expect(f.model.snapshot().nodes[0]?.overlay?.latest_attempt).toBe(
          "Succeeded",
        ),
      { timeout: 2000 },
    );
    expect(read).toHaveBeenCalledTimes(2);
    expect(f.model.snapshot().message).toBe("");
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
    expect(f.model.snapshot().nodes[0]?.overlay).toBeUndefined();
  } finally {
    f.dispose();
    read.mockRestore();
    vi.useRealTimers();
  }
});
