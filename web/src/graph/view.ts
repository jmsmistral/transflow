import type {
  ApiLineageNodeV1,
  GraphEdgeV1,
  GraphViewV1,
} from "../generated/contracts";
import { same } from "../api/validate";
export interface Visual {
  nodes: readonly ApiLineageNodeV1[];
  edges: readonly GraphEdgeV1[];
  positions: Record<string, { x: number; y: number }>;
  selected: readonly string[];
  viewport: { x: number; y: number; zoom: number };
  colour: GraphViewV1["colour"];
}
/** Bounded visual snapshots only: no service, build, schedule or catalogue mutation handle. */
export class ViewHistory {
  private past: Visual[] = [];
  private future: Visual[] = [];
  current: Visual | null = null;
  observe(next: Visual): void {
    if (same(this.current, next)) return;
    if (this.current) this.past = [...this.past.slice(-49), this.current];
    this.current = next;
    this.future = [];
  }
  reset(next: Visual): void {
    this.current = next;
    this.past = [];
    this.future = [];
  }
  get canUndo(): boolean {
    return this.past.length > 0;
  }
  get canRedo(): boolean {
    return this.future.length > 0;
  }
  undo(): Visual | null {
    const next = this.past.pop();
    if (!next || !this.current) return null;
    this.future.push(this.current);
    this.current = next;
    return next;
  }
  redo(): Visual | null {
    const next = this.future.pop();
    if (!next || !this.current) return null;
    this.past.push(this.current);
    this.current = next;
    return next;
  }
}

/** Reopening always centres the saved node bounds at 100%; camera movement is transient. */
export function openingViewport(
  datasets: GraphViewV1["datasets"],
  width: number,
  height: number,
) {
  if (!datasets.length) return { x: 0, y: 0, zoom: 1 };
  const xs = datasets.map((n) => n.position.x),
    ys = datasets.map((n) => n.position.y);
  return {
    x: width / 2 - (Math.min(...xs) + Math.max(...xs) + 250) / 2,
    y: height / 2 - (Math.min(...ys) + Math.max(...ys) + 40) / 2,
    zoom: 1,
  };
}
