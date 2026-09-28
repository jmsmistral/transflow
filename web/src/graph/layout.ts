import type { ElkNode } from "elkjs/lib/elk-api";
import type { GraphEdgeV1, GraphNodeV1 } from "../generated/contracts";
import { edgeId } from "./model";
export interface Position {
  x: number;
  y: number;
}
export interface LayoutRequest {
  id: number;
  graph: ElkNode;
}
export type LayoutReply =
  | { id: number; positions: Record<string, Position>; error?: never }
  | { id: number; error: string; positions?: never };
export function layoutGraph(
  nodes: readonly GraphNodeV1[],
  edges: readonly GraphEdgeV1[],
): ElkNode {
  const ids = new Set(nodes.map((node) => node.identity));
  const sorted = [...nodes].sort((a, b) =>
    a.identity < b.identity ? -1 : a.identity > b.identity ? 1 : 0,
  );
  return {
    id: "view",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "RIGHT",
      "elk.spacing.nodeNode": "48",
      "elk.layered.spacing.nodeNodeBetweenLayers": "90",
      "elk.randomSeed": "1",
    },
    children: sorted.map((node) => ({
      id: node.identity,
      width: 250,
      height: 40,
    })),
    edges: edges
      .filter((edge) => ids.has(edge.parent) && ids.has(edge.consumer))
      .map((edge) => ({
        id: edgeId(edge),
        sources: [edge.parent],
        targets: [edge.consumer],
      }))
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)),
  };
}
/** Layout suggestions never overwrite a manual pin, even when dragged during a worker run. */
export function positionsWithPins(
  positions: Record<string, Position>,
  pins: ReadonlyMap<string, Position>,
): Record<string, Position> {
  return Object.fromEntries(
    Object.entries(positions).map(([id, position]) => [
      id,
      pins.get(id) ?? position,
    ]),
  );
}
