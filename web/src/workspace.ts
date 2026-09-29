import type {
  ApiCapabilitiesV1,
  ApiContextV1,
  ApiDatasetV1,
  ApiDatasetsV1,
  ApiMetadataPageV1,
  ApiVersionsV1,
} from "./generated/contracts";
import { ApiFailure, Client, type Contracts, type Query } from "./api/client";
import { EventCursor } from "./api/events";
import { same } from "./api/validate";
import { follow } from "./api/stream";

export interface Selection {
  branch: string;
  fallback?: readonly string[];
  dataset?: string;
  origin?: string;
  version?: string;
  plan?: string;
  cursor?: string;
}
export interface Snapshot {
  context: ApiContextV1;
  datasets: ApiDatasetsV1;
  branches: ApiMetadataPageV1;
  dataset: ApiDatasetV1 | null;
  versions: ApiVersionsV1 | null;
}
export type WorkspaceState =
  | { kind: "disconnected"; message: string; selection: Selection }
  | { kind: "connecting" | "loading"; selection: Selection }
  | { kind: "failed"; message: string; selection: Selection }
  | { kind: "ready"; selection: Selection; value: Snapshot };
export function queryFor(selection: Selection): Query {
  return {
    branch: selection.branch,
    ...(!selection.plan && !selection.version && selection.fallback
      ? { fallback: JSON.stringify(selection.fallback) }
      : {}),
    ...(selection.plan ? { plan: selection.plan } : {}),
    ...(selection.version && selection.dataset
      ? {
          version: selection.version,
          dataset: selection.dataset,
          origin_workspace: selection.origin ?? "",
        }
      : {}),
  };
}
function matchesSelection(
  context: ApiContextV1,
  selection: Selection,
  workspace: string,
): boolean {
  return (
    context.workspace === workspace &&
    context.branch === selection.branch &&
    (selection.version
      ? context.selection.id === selection.version &&
        context.selection.kind ===
          (selection.origin === workspace ? "local_version" : "foreign_version")
      : selection.plan
        ? context.selection.kind === "plan" &&
          context.selection.id === selection.plan
        : context.selection.kind === "retained_current")
  );
}
/** One epoch owns all visible read models. Abort saves work; epoch checks provide correctness. */
export class Workspace {
  private state: WorkspaceState = {
    kind: "disconnected",
    message:
      "Open this interface through transflow serve --ui-dir <built-ui> --open.",
    selection: { branch: "master" },
  };
  private listeners = new Set<() => void>();
  private request: AbortController | undefined;
  private connection: AbortController | undefined;
  private epoch = 0;
  private inspection = 0;
  private pending: Promise<void> | undefined;
  private capabilities: ApiCapabilitiesV1 | undefined;
  private cursor: EventCursor | undefined;
  constructor(readonly client = new Client()) {}
  snapshot = (): WorkspaceState => this.state;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  private set(state: WorkspaceState): void {
    this.state = state;
    for (const listener of this.listeners) listener();
  }
  async connect(code: string): Promise<void> {
    this.dispose();
    const connection = new AbortController();
    this.connection = connection;
    this.set({ kind: "connecting", selection: this.state.selection });
    try {
      const capabilities = await this.client.connect(code, connection.signal);
      if (connection.signal.aborted) return;
      this.capabilities = capabilities;
      this.cursor = new EventCursor(capabilities.workspace);
      await this.select(this.state.selection);
      if (this.state.kind === "ready") void this.watch(connection.signal);
    } catch (error) {
      if (!connection.signal.aborted) this.fail(error);
    }
  }
  select(selection: Selection): Promise<void> {
    const state = this.state;
    const sameContext =
      state.kind === "ready" &&
      selection.dataset &&
      same(queryFor(selection), queryFor(state.selection)) &&
      selection.cursor === state.selection.cursor;
    const pending = sameContext
      ? this.inspect(selection)
      : this.load(selection);
    this.pending = pending;
    return pending;
  }
  /** Event resynchronization can supersede a selection load; await the current load. */
  async settled(): Promise<void> {
    while (this.state.kind === "loading" && this.pending) await this.pending;
  }
  /** Dataset-only selection leaves the current canvas and catalogue mounted. */
  private async inspect(selection: Selection): Promise<void> {
    const initial = this.state;
    if (initial.kind !== "ready" || !selection.dataset) return;
    const generation = ++this.inspection,
      epoch = this.epoch;
    try {
      const base = `/api/v1/datasets/${encodeURIComponent(selection.dataset)}`;
      const query = {
        origin_workspace: selection.origin ?? initial.value.context.workspace,
      };
      const [dataset, versions] = await Promise.all([
        this.read("ApiDatasetV1", base, query),
        this.read("ApiVersionsV1", `${base}/versions`, {
          ...query,
          limit: "50",
        }),
      ]);
      if (
        generation !== this.inspection ||
        epoch !== this.epoch ||
        this.state.kind !== "ready"
      )
        return;
      if (
        dataset.dataset_id !== selection.dataset ||
        dataset.workspace_id !== query.origin_workspace
      )
        throw new ApiFailure(
          "conflict",
          "Dataset metadata does not match the selection.",
        );
      this.set({
        kind: "ready",
        selection: { ...selection },
        value: { ...this.state.value, dataset, versions },
      });
    } catch (error) {
      if (generation === this.inspection && epoch === this.epoch)
        this.fail(error);
    }
  }
  private async load(selection: Selection): Promise<void> {
    this.request?.abort();
    const epoch = ++this.epoch,
      request = new AbortController();
    this.request = request;
    this.set({ kind: "loading", selection: { ...selection } });
    if (!this.capabilities) {
      this.fail(
        new ApiFailure(
          "expired",
          "Open a fresh coordinator launch link to connect.",
        ),
      );
      return;
    }
    const current = (): boolean =>
      epoch === this.epoch && !request.signal.aborted;
    try {
      const query = queryFor(selection);
      const result = await this.client.read(
        "ApiContextV1",
        "/api/v1/context",
        query,
        request.signal,
      );
      if (!current()) return;
      const context = result.data;
      if (
        !result.context ||
        !same(result.context, context) ||
        !matchesSelection(context, selection, this.capabilities.workspace)
      )
        throw new ApiFailure(
          "conflict",
          "The response does not match the selected workspace context.",
        );
      const limit = String(
        Math.min(
          50,
          this.capabilities.limits.page_default,
          this.capabilities.limits.page_max,
        ),
      );
      const [datasets, branches, dataset, versions] = await Promise.all([
        this.client.read(
          "ApiDatasetsV1",
          "/api/v1/datasets",
          {
            ...query,
            limit,
            ...(selection.cursor ? { cursor: selection.cursor } : {}),
          },
          request.signal,
          context,
        ),
        this.client.read(
          "ApiMetadataPageV1",
          "/api/v1/branches",
          { ...query, limit },
          request.signal,
          context,
        ),
        selection.dataset
          ? this.client.read(
              "ApiDatasetV1",
              `/api/v1/datasets/${encodeURIComponent(selection.dataset)}`,
              {
                ...query,
                origin_workspace: selection.origin ?? context.workspace,
              },
              request.signal,
              context,
            )
          : null,
        selection.dataset
          ? this.client.read(
              "ApiVersionsV1",
              `/api/v1/datasets/${encodeURIComponent(selection.dataset)}/versions`,
              {
                ...query,
                origin_workspace: selection.origin ?? context.workspace,
                limit,
              },
              request.signal,
              context,
            )
          : null,
      ]);
      if (!current()) return;
      if (
        dataset &&
        (dataset.data.dataset_id !== selection.dataset ||
          dataset.data.workspace_id !== selection.origin)
      )
        throw new ApiFailure(
          "conflict",
          "The dataset identity changed. Refresh before continuing.",
        );
      const allBranches = [...branches.data.entries];
      let branchCursor = branches.data.next_cursor;
      const seen = new Set<string>();
      while (branchCursor) {
        if (seen.has(branchCursor)) throw new Error("Repeated branch cursor");
        seen.add(branchCursor);
        const page = await this.client.read(
          "ApiMetadataPageV1",
          "/api/v1/branches",
          { ...query, limit: "200", cursor: branchCursor },
          request.signal,
          context,
        );
        if (!current()) return;
        allBranches.push(...page.data.entries);
        branchCursor = page.data.next_cursor;
      }
      this.set({
        kind: "ready",
        selection: { ...selection },
        value: {
          context,
          datasets: datasets.data,
          branches: { entries: allBranches, next_cursor: null },
          dataset: dataset?.data ?? null,
          versions: versions?.data ?? null,
        },
      });
    } catch (error) {
      if (current()) {
        request.abort();
        this.fail(error);
      }
    }
  }
  /** Inspector reads share the selection epoch even when a transport ignores cancellation. */
  async read<K extends keyof Contracts>(
    name: K,
    path: string,
    extra: Query = {},
    cancellation?: AbortSignal,
  ): Promise<Contracts[K]> {
    if (this.state.kind !== "ready" || !this.request)
      throw new ApiFailure(
        "conflict",
        "Load a workspace context before opening an inspector.",
      );
    const epoch = this.epoch;
    const signal = cancellation
      ? AbortSignal.any([this.request.signal, cancellation])
      : this.request.signal;
    const { context } = this.state.value;
    const result = await this.client.read(
      name,
      path,
      { ...extra, ...queryFor(this.state.selection) },
      signal,
      context,
    );
    if (epoch !== this.epoch || signal.aborted)
      throw new DOMException("The inspector context changed", "AbortError");
    return result.data;
  }
  private fail(error: unknown): void {
    this.set({
      kind:
        error instanceof ApiFailure &&
        ["disconnected", "expired"].includes(error.kind)
          ? "disconnected"
          : "failed",
      selection: this.state.selection,
      message:
        error instanceof ApiFailure
          ? error.message
          : "The coordinator response could not be verified. Refresh or inspect its diagnostics.",
    });
  }
  refresh = async (): Promise<void> => {
    const selection = { ...this.state.selection };
    delete selection.cursor;
    const pending = this.load(selection);
    this.pending = pending;
    await pending;
  };
  reconnect = (): void => {
    if (!this.connection || this.connection.signal.aborted || !this.cursor)
      return;
    const previous = this.connection;
    previous.abort();
    this.connection = new AbortController();
    void this.refresh().then(() => {
      if (this.state.kind === "ready" && this.connection)
        void this.watch(this.connection.signal);
    });
  };
  private async watch(signal: AbortSignal): Promise<void> {
    if (!this.cursor) return;
    try {
      while (!signal.aborted) {
        await follow(this.client, this.cursor, signal, async () => {
          await this.refresh();
          while (this.state.kind === "loading" && !signal.aborted)
            await this.pending;
          if (signal.aborted) return;
          if (this.state.kind !== "ready")
            throw new Error("Read models did not resynchronize");
        });
        // Normal 60-second stream rollover is resumable; errors require an explicit reconnect.
        if (!signal.aborted)
          await new Promise<void>((resolve) => {
            const finish = (): void => {
              signal.removeEventListener("abort", abort);
              resolve();
            };
            const timer = setTimeout(finish, 500);
            const abort = (): void => {
              clearTimeout(timer);
              finish();
            };
            signal.addEventListener("abort", abort, { once: true });
          });
      }
    } catch (error) {
      if (!signal.aborted) {
        this.request?.abort();
        ++this.epoch;
        this.fail(error);
      }
    }
  }
  dispose(): void {
    this.connection?.abort();
    this.request?.abort();
    ++this.epoch;
  }
}
