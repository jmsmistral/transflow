import type {
  ApiLineageV1,
  GraphEdgeV1,
  ApiLineageNodeV1,
} from "../generated/contracts";
import { ApiFailure, type Query } from "../api/client";
import type { Workspace } from "../workspace";

export interface Exploration {
  start: string;
  direction: "upstream" | "downstream";
  depth: string;
  end?: string;
}
export interface GraphState {
  context: string;
  nodes: readonly ApiLineageNodeV1[];
  edges: readonly GraphEdgeV1[];
  queryNodes: ReadonlySet<string>;
  pathNodes: ReadonlySet<string> | null;
  page: ApiLineageV1 | null;
  query: Exploration | null;
  busy: boolean;
  message: string;
}
export const edgeId = (edge: GraphEdgeV1): string =>
  JSON.stringify([edge.parent, edge.consumer, edge.alias]);
export const label = (
  node: Pick<ApiLineageNodeV1, "paths" | "identity">,
): string => node.paths[0] ?? node.identity;
export function depthQuery(value: string): Query {
  if (value === "") return {};
  if (!/^\d+$/.test(value) || BigInt(value) > 18446744073709551615n)
    throw new Error(
      "Depth must be a nonnegative whole number, or blank for all reachable datasets.",
    );
  return { depth: value };
}
/** One bounded page per action. Context and query generations fence abort-ignoring transports. */
export class GraphModel {
  private state: GraphState = {
    context: "",
    nodes: [],
    edges: [],
    queryNodes: new Set(),
    pathNodes: null,
    page: null,
    query: null,
    busy: false,
    message: "",
  };
  private listeners = new Set<() => void>();
  private request: AbortController | undefined;
  private generation = 0;
  constructor(private readonly workspace: Workspace) {}
  snapshot = (): GraphState => this.state;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  private set(patch: Partial<GraphState>): void {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  connect = (): (() => void) => {
    const sync = (): void => {
      const state = this.workspace.snapshot();
      this.cancel();
      if (state.kind !== "ready") return;
      const context = state.value.context.fingerprint;
      if (this.state.context !== context)
        this.set({
          context,
          nodes: [],
          edges: [],
          queryNodes: new Set(),
          pathNodes: null,
          page: null,
          query: null,
          message: "",
        });
      // Catalogue selection adds a root, never replaces the existing visual exploration.
      const root = state.value.dataset;
      if (
        root &&
        state.value.context.graph &&
        !this.state.nodes.some((node) => node.paths.includes(root.path))
      )
        void this.explore({
          start: root.path,
          direction: "upstream",
          depth: "0",
        });
    };
    const unsubscribe = this.workspace.subscribe(sync);
    sync();
    return () => {
      unsubscribe();
      this.cancel();
    };
  };
  cancel = (): void => {
    this.request?.abort();
    this.generation++;
    if (this.state.busy) this.set({ busy: false });
  };
  private async connections(
    nodes: readonly ApiLineageNodeV1[],
    signal: AbortSignal,
  ): Promise<GraphEdgeV1[]> {
    const visible = new Set(nodes.map((n) => n.identity));
    const edges = new Map<string, GraphEdgeV1>();
    for (let offset = 0; offset < nodes.length; offset += 100) {
      const paths = nodes.slice(offset, offset + 100).map(label);
      let cursor: string | null = null;
      const seen = new Set<string>();
      do {
        const page: ApiLineageV1 = await this.workspace.read(
          "ApiLineageV1",
          "/api/v1/lineage",
          {
            connections: JSON.stringify(paths),
            limit: "100",
            ...(cursor ? { cursor } : {}),
          },
          signal,
        );
        if (signal.aborted) return [];
        for (const edge of page.edges)
          if (visible.has(edge.parent) && visible.has(edge.consumer))
            edges.set(edgeId(edge), edge);
        cursor = page.next_cursor;
        if (cursor) {
          if (seen.has(cursor))
            throw new Error(
              "Connection page cursor repeated. Retry the graph action.",
            );
          seen.add(cursor);
        }
      } while (cursor);
    }
    return [...edges.values()];
  }
  async explore(
    query: Exploration,
    more = false,
    allowLarge = false,
  ): Promise<void> {
    this.cancel();
    const generation = this.generation;
    const request = new AbortController();
    this.request = request;
    const cursor = more ? this.state.page?.next_cursor : null;
    if (more && !cursor) return;
    this.set({ busy: true, message: "" });
    try {
      const page = await this.workspace.read(
        "ApiLineageV1",
        "/api/v1/lineage",
        {
          start: query.start,
          direction: query.end ? "downstream" : query.direction,
          ...(query.end ? { end: query.end } : depthQuery(query.depth)),
          limit: "100",
          // Explicit traversal actions authorize a bounded first batch, never an automatic drain.
          expand: "true",
          ...(cursor ? { cursor } : {}),
        },
        request.signal,
      );
      if (generation !== this.generation || request.signal.aborted) return;
      const nodes = new Map(
        this.state.nodes.map((node) => [node.identity, node]),
      );
      for (const node of page.nodes) nodes.set(node.identity, node);
      if (!allowLarge && nodes.size > 500) {
        this.set({
          busy: false,
          message:
            "This batch would exceed 500 visible datasets. Enable larger views, then retry the action.",
        });
        return;
      }
      const edges = new Map(
        this.state.edges.map((edge) => [edgeId(edge), edge]),
      );
      for (const edge of page.edges) edges.set(edgeId(edge), edge);
      for (const edge of await this.connections(
        [...nodes.values()],
        request.signal,
      ))
        edges.set(edgeId(edge), edge);
      if (generation !== this.generation || request.signal.aborted) return;
      this.set({
        nodes: [...nodes.values()],
        edges: [...edges.values()],
        queryNodes: new Set([
          ...(more ? this.state.queryNodes : []),
          ...page.nodes.map((node) => node.identity),
        ]),
        pathNodes: query.end
          ? new Set([
              ...(more ? (this.state.pathNodes ?? []) : []),
              ...page.nodes.map((node) => node.identity),
            ])
          : null,
        page,
        query,
        busy: false,
        message:
          query.end && page.total_nodes === 0
            ? "No directed path exists between these datasets in this captured graph."
            : "",
      });
    } catch (error) {
      if (generation !== this.generation || request.signal.aborted) return;
      this.set({
        busy: false,
        message:
          error instanceof ApiFailure || error instanceof Error
            ? error.message
            : "The graph could not be loaded.",
      });
    }
  }
  neighbours(
    node: ApiLineageNodeV1,
    direction: Exploration["direction"],
  ): Set<string> {
    const visible = new Set(this.state.nodes.map((n) => n.identity));
    return new Set(
      this.state.edges.flatMap((e) =>
        direction === "upstream"
          ? e.consumer === node.identity && visible.has(e.parent)
            ? [e.parent]
            : []
          : e.parent === node.identity && visible.has(e.consumer)
            ? [e.consumer]
            : [],
      ),
    );
  }
  expanded(
    node: ApiLineageNodeV1,
    direction: Exploration["direction"],
  ): boolean {
    const count = BigInt(
      direction === "upstream" ? node.parent_count : node.child_count,
    );
    return (
      count > 0n && BigInt(this.neighbours(node, direction).size) === count
    );
  }
  /** One explicit neighbour action drains bounded pages; zoom is owned solely by the view. */
  async toggle(
    node: ApiLineageNodeV1,
    direction: Exploration["direction"],
  ): Promise<Set<string> | undefined> {
    if (this.expanded(node, direction)) {
      this.remove(this.neighbours(node, direction));
      return new Set([node.identity]);
    }
    this.cancel();
    const generation = this.generation,
      request = new AbortController();
    this.request = request;
    this.set({ busy: true, message: "" });
    const nodes = new Map(this.state.nodes.map((n) => [n.identity, n]));
    const edges = new Map(this.state.edges.map((e) => [edgeId(e), e]));
    const selected = new Set<string>();
    const queryNodes = new Set<string>();
    let cursor: string | null = null;
    const seen = new Set<string>();
    try {
      let page: ApiLineageV1;
      do {
        page = await this.workspace.read(
          "ApiLineageV1",
          "/api/v1/lineage",
          {
            start: label(node),
            direction,
            depth: "1",
            limit: "100",
            expand: "true",
            ...(cursor ? { cursor } : {}),
          },
          request.signal,
        );
        if (request.signal.aborted || generation !== this.generation) return;
        for (const item of page.nodes) {
          nodes.set(item.identity, item);
          queryNodes.add(item.identity);
          if (item.identity !== node.identity) selected.add(item.identity);
        }
        for (const edge of page.edges) edges.set(edgeId(edge), edge);
        cursor = page.next_cursor;
        if (cursor) {
          if (seen.has(cursor))
            throw new Error("The graph page cursor repeated. Retry expansion.");
          seen.add(cursor);
        }
      } while (cursor);
      for (const edge of await this.connections(
        [...nodes.values()],
        request.signal,
      ))
        edges.set(edgeId(edge), edge);
      if (generation !== this.generation || request.signal.aborted) return;
      this.set({
        nodes: [...nodes.values()],
        edges: [...edges.values()],
        queryNodes,
        pathNodes: null,
        page,
        query: { start: label(node), direction, depth: "1" },
        busy: false,
        message: "",
      });
      return selected;
    } catch (error) {
      if (!request.signal.aborted && generation === this.generation)
        this.set({
          busy: false,
          message:
            error instanceof Error ? error.message : "Expansion failed. Retry.",
        });
      return undefined;
    }
  }
  remove(ids: ReadonlySet<string>): void {
    this.cancel();
    this.set({
      nodes: this.state.nodes.filter((node) => !ids.has(node.identity)),
      edges: this.state.edges.filter(
        (edge) => !ids.has(edge.parent) && !ids.has(edge.consumer),
      ),
      queryNodes: new Set(),
      pathNodes: null,
      page: null,
      query: null,
    });
  }
}
