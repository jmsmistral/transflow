import { createLayoutWorker } from "./worker";
import type { GraphState } from "./model";
import type { GraphEdgeV1, GraphNodeV1 } from "../generated/contracts";
import {
  layoutGraph,
  type LayoutReply,
  type LayoutRequest,
  type Position,
} from "./layout";
interface State {
  positions: Record<string, Position>;
  busy: boolean;
  error: string;
}
export interface LayoutWorker {
  onmessage: ((event: MessageEvent<LayoutReply>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  postMessage(message: LayoutRequest): void;
  terminate(): void;
}
/** Cancel obsolete CPU work as well as ignoring late replies. One worker owns one layout. */
export class Positions {
  private state: State = {
    positions: {},
    busy: false,
    error: "",
  };
  private worker: LayoutWorker | undefined;
  private generation = 0;
  private listeners = new Set<() => void>();
  constructor(
    private readonly create: () => LayoutWorker = createLayoutWorker,
  ) {}
  snapshot = (): State => this.state;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  private set(patch: Partial<State>): void {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  move(id: string, position: Position): void {
    this.stop();
    this.set({
      positions: { ...this.state.positions, [id]: position },
      busy: false,
    });
  }
  /** Membership changes never rearrange existing positions or invoke the layout worker. */
  sync(
    nodes: readonly GraphNodeV1[],
    edges: readonly GraphEdgeV1[] = [],
    placement: GraphState["placement"] = null,
  ): void {
    const positions: Record<string, Position> = {};
    for (const node of nodes) {
      const existing = this.state.positions[node.identity];
      if (existing) positions[node.identity] = existing;
    }
    let slot = 0;
    const pending = nodes.filter((n) => !positions[n.identity]);
    while (pending.length) {
      const neighbours = (id: string) =>
        edges.flatMap((e) => {
          const other =
            placement?.direction === "upstream"
              ? e.parent === id
                ? e.consumer
                : null
              : e.consumer === id
                ? e.parent
                : null;
          return other && positions[other] ? [positions[other]] : [];
        });
      const linked = placement
        ? pending.findIndex((n) => neighbours(n.identity).length > 0)
        : 0;
      const node = pending.splice(Math.max(0, linked), 1)[0];
      if (!node) break;
      const related = placement ? neighbours(node.identity) : [];
      const anchors = related.length
        ? related
        : (placement?.roots.flatMap((id) =>
            positions[id] ? [positions[id]] : [],
          ) ?? []);
      let candidate: Position;
      if (placement && anchors.length) {
        candidate = {
          x:
            placement.direction === "upstream"
              ? Math.min(...anchors.map((p) => p.x)) - 340
              : Math.max(...anchors.map((p) => p.x)) + 340,
          y: anchors[0]?.y ?? 100,
        };
        while (
          Object.values(positions).some(
            (p) =>
              Math.abs(p.x - candidate.x) < 270 &&
              Math.abs(p.y - candidate.y) < 60,
          )
        )
          candidate.y += 90;
      } else {
        do {
          candidate = {
            x: 60 + (slot % 4) * 300,
            y: 100 + Math.floor(slot / 4) * 90,
          };
          slot++;
        } while (
          Object.values(positions).some(
            (p) =>
              Math.abs(p.x - candidate.x) < 270 &&
              Math.abs(p.y - candidate.y) < 60,
          )
        );
      }
      positions[node.identity] = candidate;
    }
    this.set({ positions });
  }
  run(nodes: readonly GraphNodeV1[], edges: readonly GraphEdgeV1[]): void {
    this.stop();
    const id = this.generation;
    const ids = new Set(nodes.map((node) => node.identity));
    this.set({
      busy: nodes.length > 0,
      error: "",
      positions: Object.fromEntries(
        Object.entries(this.state.positions).filter(([key]) => ids.has(key)),
      ),
    });
    if (!nodes.length) return;
    try {
      const worker = this.create();
      this.worker = worker;
      worker.onmessage = ({ data }) => {
        if (id !== this.generation || data.id !== id) return;
        worker.terminate();
        if (data.error !== undefined)
          this.set({ busy: false, error: data.error });
        else
          this.set({
            busy: false,
            positions: data.positions,
          });
      };
      worker.onerror = () => {
        if (id !== this.generation) return;
        worker.terminate();
        this.set({
          busy: false,
          error:
            "Layout worker failed. Retry layout; manual node positioning remains available.",
        });
      };
      worker.postMessage({ id, graph: layoutGraph(nodes, edges) });
    } catch {
      this.set({
        busy: false,
        error:
          "Layout worker is unavailable. Retry layout; manual node positioning remains available.",
      });
    }
  }
  stop = (): void => {
    this.generation++;
    this.worker?.terminate();
    this.worker = undefined;
  };
}
