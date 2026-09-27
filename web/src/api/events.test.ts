import { describe, expect, it } from "vitest";
import { EventCursor } from "./events";
import type { ApiEventV1 } from "../generated/contracts";
const event = (sequence: string, id = sequence): ApiEventV1 => ({
  sequence,
  id,
  workspace: "workspace",
  type: "job.state",
  context: {},
  payload: { job: "job", state: "RUNNING" },
  causation: null,
  correlation: null,
  timestamp_us: "1",
});
describe("committed event resume", () => {
  it("reconnects without applying duplicate or reordered deliveries", () => {
    const cursor = new EventCursor("workspace");
    let applied = 0;
    for (const e of [
      event("1"),
      event("2"),
      event("2"),
      event("1"),
      event("3"),
    ]) {
      if (cursor.accept(e)) applied++;
    }
    expect(applied).toBe(3);
    const reconnect = new EventCursor("workspace", cursor.cursor);
    expect(reconnect.accept(event("3"))).toBe(false);
    expect(reconnect.accept(event("4"))).toBe(true);
  });
  it("requires explicit resync and never mixes workspaces", () => {
    const cursor = new EventCursor("workspace");
    expect(cursor.accept(event("9007199254740993", "first"))).toBe(true);
    expect(() => cursor.accept(event("9007199254740994", "first"))).toThrow(
      /resync/,
    );
    expect(() =>
      cursor.accept({ ...event("9007199254740994"), workspace: "other" }),
    ).toThrow(/workspace/);
    cursor.resynchronized("9007199254740995");
    expect(cursor.accept(event("9007199254740994"))).toBe(false);
    expect(() => new EventCursor("workspace", "-1")).toThrow();
  });
});
