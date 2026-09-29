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
  viewport: GraphViewV1["viewport"];
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
