import { GraphCache, type Scope } from "./cache";
import type {
  ApiLineageV1,
  GraphEdgeV1,
  ApiLineageNodeV1,
} from "../generated/contracts";
import { ApiFailure, type Query } from "../api/client";
import { visualContext as contextIdentity, type Workspace } from "../workspace";

export interface Exploration {
  start: string;
  direction: "upstream" | "downstream";
  depth: string;
  end?: string;
}
export interface GraphState {
  context: string;
  /** Camera and selection survive timing-only context revisions. */
  visualContext?: string;
  placement: {
    roots: readonly string[];
    direction: Exploration["direction"];
  } | null;
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
    placement: null,
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
  private pendingAdds = new Set<string>();
  private viewSelector = "";
  private cache: GraphCache;
  constructor(private readonly workspace: Workspace) {
    this.cache = new GraphCache(workspace);
  }
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
      if (state.kind !== "ready") {
        this.cancel();
        this.cache.reset();
        return;
      }
      const changed = state.value.context.fingerprint !== this.state.context;
      if (changed) {
        this.cancel();
        this.cache.reset();
      }
      const context = state.value.context.fingerprint;
      const visualContext = contextIdentity(state.value.context);
      const selector = JSON.stringify([
        state.value.context.workspace,
        state.value.context.branch,
        state.value.context.selection,
        state.value.context.fallback_policy,
      ]);
      // A build may publish a new capture/registry. Rebind visible identities in
      // the same branch view while retaining the camera and node positions.
      const timingOnly =
        changed &&
        (this.state.visualContext === visualContext ||
          (state.value.context.selection.kind === "retained_current" &&
            selector === this.viewSelector));
      this.viewSelector = selector;
      if (timingOnly) {
        const identities = this.state.nodes.map((node) => node.identity);
        this.set({
          context,
          nodes: this.state.nodes.map((node) => {
            const fresh = { ...node };
            delete fresh.overlay;
            return { ...fresh, publication: "unknown" };
          }),
        });
        if (identities.length)
          void this.reopen(identities).catch((error) => {
            if (this.state.context === context)
              this.set({
                message:
                  error instanceof Error
                    ? error.message
                    : "Updated graph metadata is unavailable.",
              });
          });
      }
      if (this.state.context !== context)
        this.set({
          context,
          visualContext,
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
        changed &&
        root &&
        state.value.context.graph &&
        !this.state.nodes.some((node) => node.paths.includes(root.path))
      )
        void this.add(root.path);
    };
    const unsubscribe = this.workspace.subscribe(sync);
    sync();
    return () => {
      unsubscribe();
      this.cancel();
      this.cache.reset();
    };
  };
  /** Restore visual membership only; metadata always comes from the selected coordinator context. */
  async reopen(ids: readonly string[]): Promise<void> {
    this.cancel();
    const generation = this.generation;
    const nodes: ApiLineageNodeV1[] = [],
      edges = new Map<string, GraphEdgeV1>();
    for (let offset = 0; offset < ids.length; offset += 100) {
      const scope = await this.cache.lookup(ids.slice(offset, offset + 100));
      if (generation !== this.generation)
        throw new Error("The graph context changed while opening the view.");
      nodes.push(...scope.nodes);
      for (const edge of scope.edges) edges.set(edgeId(edge), edge);
    }
    if (ids.some((id) => !nodes.some((n) => n.identity === id)))
      throw new Error(
        "A saved dataset is unavailable in this context; the current view was preserved.",
      );
    this.restore({ nodes, edges: [...edges.values()] });
  }
  restore(scope: {
    nodes: readonly ApiLineageNodeV1[];
    edges: readonly GraphEdgeV1[];
  }): void {
    this.cancel();
    this.set({
      nodes: scope.nodes,
      edges: scope.edges,
      placement: null,
      busy: false,
      message: "",
      pathNodes: null,
      queryNodes: new Set(),
    });
  }
  prefetch(paths: readonly string[]): void {
    void this.cache.warm(paths).catch(() => {});
  }
  private accept(
    scope: Scope,
    placement: GraphState["placement"] = null,
  ): void {
    const nodes = new Map(this.state.nodes.map((n) => [n.identity, n]));
    for (const node of scope.nodes) nodes.set(node.identity, node);
    const replaced = new Set(scope.nodes.map((n) => n.identity));
    const edges = new Map(
      this.state.edges
        .filter((e) => !replaced.has(e.consumer))
        .map((e) => [edgeId(e), e]),
    );
    for (const edge of scope.edges) edges.set(edgeId(edge), edge);
    this.set({
      nodes: [...nodes.values()],
      edges: [...edges.values()],
      placement,
      busy: this.pendingAdds.size > 0,
      message: "",
      page: null,
      query: null,
      pathNodes: null,
    });
  }
  async add(path: string): Promise<void> {
    if (this.state.nodes.some((n) => n.paths.includes(path))) return;
    if (this.pendingAdds.has(path)) return;
    if (!this.pendingAdds.size) this.cancel();
    const generation = this.generation;
    const known = this.cache.known([path]);
    if (known) {
      this.accept(known);
      return;
    }
    this.pendingAdds.add(path);
    this.set({ busy: true, message: "" });
    try {
      const scope = await this.cache.lookup([path]);
      if (generation === this.generation) {
        this.pendingAdds.delete(path);
        this.accept(scope);
      }
    } catch (error) {
      if (generation === this.generation)
        this.set({
          busy: false,
          message:
            error instanceof Error ? error.message : "Could not add dataset.",
        });
    } finally {
      if (generation === this.generation) {
        this.pendingAdds.delete(path);
        this.set({ busy: this.pendingAdds.size > 0 });
      }
    }
  }
  async addMany(paths: readonly string[]): Promise<void> {
    this.cancel();
    const generation = this.generation;
    this.set({ busy: true, message: "" });
    try {
      const requested = [...new Set(paths)].slice(0, 100);
      const scope = await this.cache.lookup(requested);
      if (generation === this.generation) {
        // Keep the plan's parent-before-child order for newly placed nodes.
        const rank = (node: ApiLineageNodeV1) =>
          requested.findIndex(
            (path) => node.identity === path || node.paths.includes(path),
          );
        this.accept(
          {
            ...scope,
            nodes: [...scope.nodes].sort((a, b) => rank(a) - rank(b)),
          },
          { roots: [], direction: "upstream" },
        );
      }
    } catch (error) {
      if (generation === this.generation)
        this.set({
          message:
            error instanceof Error
              ? error.message
              : "Could not add plan resources.",
        });
    } finally {
      if (generation === this.generation) this.set({ busy: false });
    }
  }
  cancel = (): void => {
    this.pendingAdds.clear();
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
          overlays: "true",
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
    return this.expand([node], direction, "1");
  }
  /** Explicit multi-root expansion drains bounded pages atomically within one context. */
  async expand(
    roots: readonly ApiLineageNodeV1[],
    direction: Exploration["direction"],
    depth: string,
  ): Promise<Set<string> | undefined> {
    this.cancel();
    const generation = this.generation,
      request = new AbortController();
    this.request = request;
    this.set({ busy: true, message: "" });
    try {
      depthQuery(depth);
      const scope =
        this.cache.neighbourhood(roots, direction, depth) ??
        (await this.cache.expansion(roots, direction, depth, request.signal));
      if (generation !== this.generation || request.signal.aborted) return;
      this.accept(scope, { roots: roots.map((n) => n.identity), direction });
      const rootIds = new Set(roots.map((n) => n.identity));
      return new Set(
        scope.nodes
          .filter((n) => !rootIds.has(n.identity))
          .map((n) => n.identity),
      );
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
      edges: this.state.edges.filter((edge) => !ids.has(edge.consumer)),
      queryNodes: new Set(),
      pathNodes: null,
      page: null,
      query: null,
    });
  }
}
