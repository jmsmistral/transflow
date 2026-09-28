import { createLayoutWorker } from "./worker";
import type { GraphEdgeV1, GraphNodeV1 } from "../generated/contracts";
import {
  layoutGraph,
  positionsWithPins,
  type LayoutReply,
  type LayoutRequest,
  type Position,
} from "./layout";
interface State {
  positions: Record<string, Position>;
  pins: ReadonlyMap<string, Position>;
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
    pins: new Map(),
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
    this.set({
      positions: { ...this.state.positions, [id]: position },
      pins: new Map(this.state.pins).set(id, position),
    });
  }
  unpin(ids: ReadonlySet<string>): void {
    this.set({
      pins: new Map([...this.state.pins].filter(([id]) => !ids.has(id))),
    });
  }
  run(
    nodes: readonly GraphNodeV1[],
    edges: readonly GraphEdgeV1[],
    reset = false,
  ): void {
    this.stop();
    const id = this.generation;
    const ids = new Set(nodes.map((node) => node.identity));
    this.set({
      busy: nodes.length > 0,
      error: "",
      pins: reset
        ? new Map()
        : new Map([...this.state.pins].filter(([key]) => ids.has(key))),
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
            positions: positionsWithPins(data.positions, this.state.pins),
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
