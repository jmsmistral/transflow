import type {
  ApiLineageNodeV1,
  GraphEdgeV1,
  GraphViewV1,
  ViewGroupV1,
} from "../generated/contracts";
import { same } from "../api/validate";
export interface Visual {
  nodes: readonly ApiLineageNodeV1[];
  edges: readonly GraphEdgeV1[];
  positions: Record<string, { x: number; y: number }>;
  selected: readonly string[];
  groups: GraphViewV1["groups"];
  annotations: GraphViewV1["annotations"];
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
export function groupsFor(
  nodes: readonly ApiLineageNodeV1[],
  positions: Visual["positions"],
  classify: (n: ApiLineageNodeV1) => string,
): ViewGroupV1[] {
  const groups = new Map<string, ApiLineageNodeV1[]>();
  for (const node of nodes) {
    const key = classify(node);
    groups.set(key, [...(groups.get(key) ?? []), node]);
  }
  if (groups.size > 100)
    throw new Error(
      "Choose a smaller selection: a view supports up to 100 groups.",
    );
  return [...groups].map(([name, members]) => ({
    id: crypto.randomUUID(),
    name,
    members: members.map((n) => n.identity),
    collapsed: false,
    position: {
      x: Math.min(...members.map((n) => positions[n.identity]?.x ?? 0)) - 12,
      y: Math.min(...members.map((n) => positions[n.identity]?.y ?? 0)) - 40,
    },
  }));
}
export function rollup(
  group: ViewGroupV1,
  nodes: readonly ApiLineageNodeV1[],
): string {
  const members = nodes.filter((n) => group.members.includes(n.identity));
  return `${members.length} datasets · ${members.filter((n) => n.publication === "published").length} published · ${members.filter((n) => n.publication === "missing").length} not built · ${members.filter((n) => n.publication === "unknown").length} unknown`;
}
