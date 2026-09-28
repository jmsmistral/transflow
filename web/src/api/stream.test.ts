import { expect, test, vi } from "vitest";
import { Client } from "./client";
import { EventCursor } from "./events";
import { follow } from "./stream";
import { workspaceId, datasetId } from "../test-fixtures";
function frame(type: string, id: string, data: unknown): string {
  return `event: ${type}\nid: ${id}\ndata: ${JSON.stringify(data)}\n\n`;
}
const fact = {
  sequence: "1",
  id: datasetId,
  type: "build.changed",
  payload: {},
  causation: null,
  correlation: null,
  timestamp_us: "1",
  workspace: workspaceId,
  context: {},
};
function client(frames: string): Client {
  return new Client(
    vi.fn(
      async () =>
        new Response(
          new ReadableStream({
            start(controller) {
              const bytes = new TextEncoder().encode(frames);
              for (let i = 0; i < bytes.length; i += 7)
                controller.enqueue(bytes.slice(i, i + 7));
              controller.close();
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        ),
    ),
  );
}
test("split SSE frames deduplicate facts and refetch authoritative models at checkpoint", async () => {
  const cursor = new EventCursor(workspaceId),
    refresh = vi.fn(async () => undefined);
  await follow(
    client(
      frame("fact", "1", fact) +
        frame("fact", "1", fact) +
        frame("checkpoint", "1", { workspace: workspaceId, cursor: "1" }),
    ),
    cursor,
    new AbortController().signal,
    refresh,
  );
  expect(refresh).toHaveBeenCalledOnce();
  expect(cursor.cursor).toBe("1");
});
test("replay gaps acknowledge checkpoint only after successful read-model refresh", async () => {
  const cursor = new EventCursor(workspaceId),
    body = frame("resync_required", "42", {
      workspace: workspaceId,
      cursor: "42",
    });
  await expect(
    follow(client(body), cursor, new AbortController().signal, async () => {
      throw new Error("refresh failed");
    }),
  ).rejects.toThrow("refresh failed");
  expect(cursor.cursor).toBe("0");
  await follow(
    client(body),
    cursor,
    new AbortController().signal,
    async () => undefined,
  );
  expect(cursor.cursor).toBe("42");
});
test("foreign events, mismatched IDs and truncated frames fail closed", async () => {
  for (const body of [
    frame("fact", "1", { ...fact, workspace: datasetId }),
    frame("fact", "2", fact),
    frame("fact", "1", fact).slice(0, -1),
    frame("checkpoint", "01", { workspace: workspaceId, cursor: "01" }),
  ]) {
    await expect(
      follow(
        client(body),
        new EventCursor(workspaceId),
        new AbortController().signal,
        async () => undefined,
      ),
    ).rejects.toThrow();
  }
});

test("an interrupted response body is a disconnected state even if cancel rejects", async () => {
  const disconnected = new Client(
    vi.fn(
      async () =>
        new Response(
          new ReadableStream({
            start(controller) {
              controller.error(new TypeError("network stopped"));
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        ),
    ),
  );
  await expect(
    follow(
      disconnected,
      new EventCursor(workspaceId),
      new AbortController().signal,
      async () => undefined,
    ),
  ).rejects.toMatchObject({ kind: "disconnected" });
});
