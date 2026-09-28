import { useEffect, useState, useSyncExternalStore } from "react";
import type { Workspace } from "../workspace";
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
}: {
  workspace: Workspace;
  active: boolean;
  visible: readonly string[];
  select: (entry: ApiDatasetsV1["entries"][number]) => void;
}) {
  const state = useSyncExternalStore(workspace.subscribe, workspace.snapshot);
  const fingerprint =
    state.kind === "ready" ? state.value.context.fingerprint : "";
  const [filter, setFilter] = useState("");
  const [cursor, setCursor] = useState<string | null>(null);
  const [result, setResult] = useState<{
    key: string;
    page: ApiDatasetsV1;
  } | null>(null);
  const [message, setMessage] = useState("");
  const key = JSON.stringify([fingerprint, filter, cursor]);
  useEffect(() => {
    if (!active || !fingerprint) return;
    const request = new AbortController();
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
                setResult({ key, page });
                setMessage("");
              }
            },
            (error: unknown) => {
              if (!request.signal.aborted)
                setMessage(
                  error instanceof Error
                    ? error.message
                    : "Catalogue search failed.",
                );
            },
          );
      },
      filter ? 150 : 0,
    );
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  }, [workspace, active, fingerprint, filter, cursor, key]);
  const page = result?.key === key ? result.page : null;
  return (
    <div className="catalogue-search">
      <input
        type="search"
        aria-label="Search catalogue"
        placeholder="Search datasets…"
        value={filter}
        onChange={(event) => {
          setFilter(event.target.value);
          setCursor(null);
          setMessage("");
        }}
      />
      {message && <p role="alert">{message}</p>}
      <ul
        className="dataset-list"
        aria-label="Catalogue datasets"
        aria-busy={!page && active}
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
      {!page && active && !message && <p role="status">Searching catalogue…</p>}
      {page && (
        <div className="toolbar">
          <Button disabled={!cursor} onClick={() => setCursor(null)}>
            First page
          </Button>
          <Button
            disabled={!page.next_cursor}
            onClick={() => setCursor(page.next_cursor)}
          >
            Next page
          </Button>
          <small>{page.total} datasets</small>
        </div>
      )}
    </div>
  );
}
