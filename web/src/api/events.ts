import type { ApiEventV1 } from "../generated/contracts";

/** Workspace-scoped resume state; notifications invalidate read models, not guessed job transitions. */
export class EventCursor {
  private sequence: bigint;
  private readonly seen = new Set<string>();
  constructor(
    readonly workspace: string,
    cursor = "0",
  ) {
    this.sequence = parseCursor(cursor);
  }
  get cursor(): string {
    return this.sequence.toString();
  }
  accept(event: ApiEventV1): boolean {
    if (event.workspace !== this.workspace)
      throw new Error("Event workspace changed");
    const sequence = parseCursor(event.sequence);
    if (sequence <= this.sequence) return false;
    if (this.seen.has(event.id))
      throw new Error("Event identity changed sequence; resync required");
    this.sequence = sequence;
    this.seen.add(event.id);
    if (this.seen.size > 10000) {
      const oldest = this.seen.values().next();
      if (!oldest.done) this.seen.delete(oldest.value);
    }
    return true;
  }
  /** Call only after refetching current context/read models, never to hide a replay gap. */
  resynchronized(cursor: string): void {
    this.sequence = parseCursor(cursor);
    this.seen.clear();
  }
}
function parseCursor(value: string): bigint {
  if (!/^(0|[1-9][0-9]*)$/.test(value)) throw new Error("Invalid event cursor");
  const n = BigInt(value);
  if (n > 9223372036854775807n) throw new Error("Invalid event cursor");
  return n;
}
