import { ApiFailure, Client, decode, readChunk } from "./client";
import { EventCursor } from "./events";
import { obj, str } from "./validate";

/** Fetch-based SSE preserves mandatory Origin and CSRF on the read-only POST facade. */
export async function follow(
  client: Client,
  cursor: EventCursor,
  signal: AbortSignal,
  refresh: () => Promise<void>,
): Promise<void> {
  const response = await client.response(
    "/api/v1/read",
    { path: "/api/v1/events", query: { after: cursor.cursor } },
    signal,
  );
  if (
    !response.headers.get("content-type")?.startsWith("text/event-stream") ||
    !response.body
  )
    throw new ApiFailure(
      "failed",
      "The coordinator event stream is unavailable.",
    );
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let buffer = "";
  let pending = false;
  try {
    while (!signal.aborted) {
      const { done, value } = await readChunk(reader, signal);
      buffer += done
        ? decoder.decode()
        : decoder.decode(value, { stream: true });
      if (buffer.length > 2 * 1024 * 1024)
        throw new ApiFailure("failed", "An event exceeded the supported size.");
      let boundary: number;
      while ((boundary = buffer.indexOf("\n\n")) >= 0) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        let type = "",
          data = "",
          id = "";
        for (const line of frame.split("\n")) {
          if (line.startsWith("event: ")) type = line.slice(7);
          if (line.startsWith("id: ")) id = line.slice(4);
          if (line.startsWith("data: ")) data += line.slice(6);
        }
        if (!data) continue;
        const payload: unknown = JSON.parse(data);
        if (type === "fact") {
          const fact = decode("ApiEventV1", payload);
          if (id !== fact.sequence) throw new Error("Event identity mismatch");
          if (cursor.accept(fact)) pending = true;
        } else if (type === "checkpoint" || type === "resync_required") {
          const checkpoint = obj(payload);
          if (
            checkpoint.workspace !== cursor.workspace ||
            checkpoint.cursor !== id
          )
            throw new Error("Checkpoint mismatch");
          if (type === "resync_required") {
            await refresh(); // Do not acknowledge a replay gap until authoritative models have loaded.
            cursor.resynchronized(str(checkpoint.cursor));
            return;
          }
          if (
            !/^(0|[1-9][0-9]*)$/.test(id) ||
            BigInt(id) > 9223372036854775807n ||
            BigInt(id) < BigInt(cursor.cursor)
          )
            throw new Error("Invalid checkpoint sequence");
          if (pending || BigInt(id) > BigInt(cursor.cursor)) {
            await refresh();
            if (BigInt(id) > BigInt(cursor.cursor))
              cursor.resynchronized(str(checkpoint.cursor));
            pending = false;
          }
        } else throw new Error("Unsupported event");
      }
      if (done) {
        if (buffer.trim() || pending) throw new Error("Incomplete event");
        return;
      }
    }
  } finally {
    // Preserve a read error when cancellation of that failed stream also rejects.
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
