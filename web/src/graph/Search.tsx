import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { Workspace, WorkspaceState } from "../workspace";
import type { ApiDatasetsV1 } from "../generated/contracts";
import { Button } from "../components";

/** Ordered, case-insensitive subsequence matching, shared with the API search policy. */
export function matchPositions(path: string, query: string): Set<number> {
  const wanted = [...query.toLowerCase()];
  const matched = new Set<number>();
  let index = 0;
  for (const [position, char] of [...path].entries()) {
    if (char.toLowerCase() === wanted[index]) {
      matched.add(position);
      index++;
    }
  }
  return index === wanted.length ? matched : new Set();
}
export function CatalogueSearch({
  workspace,
  active,
  visible,
  select,
  onPage,
  query,
  onQuery,
}: {
  query?: string;
  onQuery?: (query: string) => void;
  workspace: Workspace;
  active: boolean;
  visible: readonly string[];
  onPage?: (paths: readonly string[]) => void;
  select: (entry: ApiDatasetsV1["entries"][number]) => void;
}) {
  const state = useSyncExternalStore(workspace.subscribe, workspace.snapshot);
  const fingerprint =
    state.kind === "ready" ? state.value.context.fingerprint : "";
  const [localFilter, setLocalFilter] = useState("");
  const filter = query ?? localFilter;
  const setFilter = (value: string) => {
    setLocalFilter(value);
    onQuery?.(value);
  };
  return (
    <div className="catalogue-search">
      <input
        type="search"
        aria-label="Search catalogue"
        placeholder="Search datasets…"
        value={filter}
        onChange={(event) => setFilter(event.target.value)}
      />
      <CatalogueResults
        key={JSON.stringify([fingerprint, filter])}
        workspace={workspace}
        active={active}
        filter={filter}
        fingerprint={fingerprint}
        state={state}
        visible={visible}
        select={select}
        onPage={onPage}
      />
    </div>
  );
}

function CatalogueResults({
  workspace,
  active,
  filter,
  fingerprint,
  state,
  visible,
  select,
  onPage,
}: {
  workspace: Workspace;
  active: boolean;
  filter: string;
  fingerprint: string;
  state: WorkspaceState;
  visible: readonly string[];
  select: (entry: ApiDatasetsV1["entries"][number]) => void;
  onPage: ((paths: readonly string[]) => void) | undefined;
}) {
  const [cursor, setCursor] = useState<string | null>(null);
  const [result, setResult] = useState<{
    cursor: string | null;
    page: ApiDatasetsV1;
  } | null>(null);
  const [message, setMessage] = useState("");
  const [retry, setRetry] = useState(0);
  const pending = useRef(false);
  useEffect(() => {
    if (!active || !fingerprint) return;
    const request = new AbortController();
    pending.current = true;
    const timer = setTimeout(
      () => {
        void workspace
          .read(
            "ApiDatasetsV1",
            "/api/v1/datasets",
            {
              fuzzy: filter,
              limit: "50",
              ...(cursor ? { cursor } : {}),
            },
            request.signal,
          )
          .then(
            (page) => {
              if (!request.signal.aborted) {
                pending.current = false;
                setResult((old) => ({
                  cursor,
                  page: {
                    ...page,
                    entries:
                      cursor && old
                        ? [...old.page.entries, ...page.entries]
                        : page.entries,
                  },
                }));
                setMessage("");
              }
            },
            (error: unknown) => {
              if (!request.signal.aborted) {
                pending.current = false;
                setMessage(
                  error instanceof Error
                    ? error.message
                    : "Catalogue search failed.",
                );
              }
            },
          );
      },
      filter && !cursor ? 150 : 0,
    );
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  }, [workspace, active, fingerprint, filter, cursor, retry]);
  const page = result?.page;
  const loading = active && !message && (!result || result.cursor !== cursor);
  useEffect(() => {
    onPage?.(page?.entries.map((e) => e.path) ?? []);
  }, [page, onPage]);
  const loadMore = () => {
    if (!active || pending.current || message || !page?.next_cursor) return;
    pending.current = true;
    setCursor(page.next_cursor);
  };
  return (
    <>
      {message && (
        <p role="alert">
          {message}{" "}
          <Button
            onClick={() => {
              pending.current = true;
              setMessage("");
              setRetry((value) => value + 1);
            }}
          >
            Retry loading datasets
          </Button>
        </p>
      )}
      <ul
        className="dataset-list"
        aria-label="Catalogue datasets"
        aria-busy={loading}
        onScroll={(event) => {
          const list = event.currentTarget;
          if (
            list.scrollTop > 0 &&
            list.scrollHeight - list.scrollTop <= list.clientHeight * 2
          )
            loadMore();
        }}
        onWheel={(event) => {
          const list = event.currentTarget;
          if (
            event.deltaY > Math.abs(event.deltaX) &&
            list.scrollHeight <= list.clientHeight
          )
            loadMore();
        }}
      >
        {page?.entries.map((entry) => {
          const present = visible.includes(
            `dataset:${entry.workspace_id}:${entry.dataset_id}`,
          );
          const highlights = matchPositions(entry.path, filter);
          return (
            <li key={`${entry.workspace_id}:${entry.dataset_id}`}>
              <button
                aria-label={`${entry.path}, ${entry.origin} · ${entry.kind}`}
                onClick={() => select(entry)}
                aria-pressed={
                  state.selection.dataset === entry.dataset_id &&
                  state.selection.origin === entry.workspace_id
                }
              >
                <span>
                  {present && (
                    <span
                      className="in-view-dot"
                      role="img"
                      aria-label="Already in lineage"
                      title="Already in lineage"
                    />
                  )}
                  {[...entry.path].map((char, i) =>
                    highlights.has(i) ? <mark key={i}>{char}</mark> : char,
                  )}
                </span>
                <small>
                  {entry.origin} · {entry.kind}
                </small>
              </button>
            </li>
          );
        })}
      </ul>
      {page?.entries.length === 0 && <p>No datasets match this search.</p>}
      {loading && (
        <p role="status">
          {page ? "Loading more datasets…" : "Searching catalogue…"}
        </p>
      )}
      {page && (
        <small>
          {BigInt(page.total) > BigInt(page.entries.length)
            ? `${page.entries.length} of `
            : ""}
          {page.total} datasets
        </small>
      )}
    </>
  );
}
