import type {
  ApiLineageNodeV1,
  ApiLineageV1,
  GraphEdgeV1,
} from "../generated/contracts";
import type { Query } from "../api/client";
import type { Workspace } from "../workspace";
import { edgeId, label, type Exploration } from "./model";
export interface Scope {
  nodes: ApiLineageNodeV1[];
  edges: GraphEdgeV1[];
}
/** Bounded, context-owned read cache. Only complete incoming-dependency scopes are reusable. */
export class GraphCache {
  private records = new Map<
    string,
    { node: ApiLineageNodeV1; edges: GraphEdgeV1[] }
  >();
  private pending = new Map<string, Promise<Scope>>();
  private lifetime = new AbortController();
  constructor(private readonly workspace: Workspace) {}
  reset(): void {
    this.lifetime.abort();
    this.lifetime = new AbortController();
    this.records.clear();
    this.pending.clear();
  }
  private remember(scope: Scope): void {
    for (const node of scope.nodes) {
      const edges = scope.edges.filter((e) => e.consumer === node.identity);
      if (edges.length > 10000) continue;
      this.records.delete(node.identity);
      this.records.set(node.identity, { node, edges });
    }
    let edgeCount = [...this.records.values()].reduce(
      (sum, r) => sum + r.edges.length,
      0,
    );
    for (const [id, record] of this.records) {
      if (this.records.size <= 2000 && edgeCount <= 10000) break;
      edgeCount -= record.edges.length;
      this.records.delete(id);
    }
  }
  known(paths: readonly string[]): Scope | null {
    const nodes = paths.map(
      (path) =>
        [...this.records.values()].find((r) => r.node.paths.includes(path))
          ?.node,
    );
    if (nodes.some((n) => !n)) return null;
    return this.scope(nodes.filter((n): n is ApiLineageNodeV1 => !!n));
  }
  private scope(nodes: ApiLineageNodeV1[]): Scope {
    const edges = new Map<string, GraphEdgeV1>();
    for (const node of nodes)
      for (const edge of this.records.get(node.identity)?.edges ?? [])
        edges.set(edgeId(edge), edge);
    return { nodes, edges: [...edges.values()] };
  }
  neighbourhood(
    roots: readonly ApiLineageNodeV1[],
    direction: Exploration["direction"],
    depth: string,
  ): Scope | null {
    const maximum = depth === "" ? Infinity : Number(depth);
    const nodes = new Map<string, ApiLineageNodeV1>();
    const queue = roots.map((node) => ({ node, distance: 0 }));
    const allEdges = [...this.records.values()].flatMap((r) => r.edges);
    for (let index = 0; index < queue.length; index++) {
      const item = queue[index];
      if (!item || nodes.has(item.node.identity)) continue;
      const { node, distance } = item;
      if (!this.records.has(node.identity)) return null;
      nodes.set(node.identity, node);
      if (distance >= maximum || (node.external && direction === "upstream"))
        continue;
      const ids = new Set(
        allEdges
          .filter((e) =>
            direction === "upstream"
              ? e.consumer === node.identity
              : e.parent === node.identity,
          )
          .map((e) => (direction === "upstream" ? e.parent : e.consumer)),
      );
      if (
        BigInt(ids.size) !==
        BigInt(direction === "upstream" ? node.parent_count : node.child_count)
      )
        return null;
      for (const id of ids) {
        const next = this.records.get(id)?.node;
        if (!next) return null;
        queue.push({ node: next, distance: distance + 1 });
      }
    }
    return this.scope([...nodes.values()]);
  }
  async read(query: Query, cancellation?: AbortSignal): Promise<Scope> {
    const lifetime = this.lifetime;
    const signal = cancellation
      ? AbortSignal.any([lifetime.signal, cancellation])
      : lifetime.signal;
    const nodes = new Map<string, ApiLineageNodeV1>(),
      edges = new Map<string, GraphEdgeV1>();
    let cursor: string | null = null;
    const seen = new Set<string>();
    do {
      const page: ApiLineageV1 = await this.workspace.read(
        "ApiLineageV1",
        "/api/v1/lineage",
        {
          ...query,
          expand: "true",
          limit: "100",
          ...(cursor ? { cursor } : {}),
        },
        signal,
      );
      if (signal.aborted)
        throw new DOMException("Context changed", "AbortError");
      for (const node of page.nodes) nodes.set(node.identity, node);
      for (const edge of page.edges) edges.set(edgeId(edge), edge);
      cursor = page.next_cursor;
      if (cursor && seen.has(cursor))
        throw new Error("Graph cursor repeated. Retry the action.");
      if (cursor) seen.add(cursor);
    } while (cursor);
    const scope = { nodes: [...nodes.values()], edges: [...edges.values()] };
    this.remember(scope);
    return scope;
  }
  lookup(paths: readonly string[]): Promise<Scope> {
    const known = this.known(paths);
    if (known) return Promise.resolve(known);
    const key = JSON.stringify([...new Set(paths)].sort());
    const pending = this.pending.get(key);
    if (pending) return pending;
    // A click while the displayed catalogue page is warming shares that read.
    for (const [batch, work] of this.pending) {
      const batchPaths: string[] = JSON.parse(batch);
      if (paths.every((path) => batchPaths.includes(path)))
        return work.then((scope) => {
          const nodes = scope.nodes.filter((n) =>
            n.paths.some((p) => paths.includes(p)),
          );
          const ids = new Set(nodes.map((n) => n.identity));
          return {
            nodes,
            edges: scope.edges.filter((e) => ids.has(e.consumer)),
          };
        });
    }
    const work = this.read({ lookup: key });
    this.pending.set(key, work);
    void work
      .finally(() => {
        if (this.pending.get(key) === work) this.pending.delete(key);
      })
      .catch(() => {});
    return work;
  }
  async warm(paths: readonly string[]): Promise<void> {
    // Only the currently displayed catalogue page, never unbounded graph preloading.
    if (paths.length) await this.lookup(paths.slice(0, 100));
  }
  async expansion(
    roots: readonly ApiLineageNodeV1[],
    direction: Exploration["direction"],
    depth: string,
    signal: AbortSignal,
  ): Promise<Scope> {
    const nodes = new Map<string, ApiLineageNodeV1>(),
      edges = new Map<string, GraphEdgeV1>();
    for (const root of roots) {
      const part = await this.read(
        {
          start: label(root),
          direction,
          ...(depth === "" ? {} : { depth }),
          incoming: "true",
        },
        signal,
      );
      for (const n of part.nodes) nodes.set(n.identity, n);
      for (const e of part.edges) edges.set(edgeId(e), e);
    }
    return { nodes: [...nodes.values()], edges: [...edges.values()] };
  }
}
