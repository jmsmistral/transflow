import { useEffect, useRef, useState } from "react";
import type { Workspace } from "../workspace";
import type { ApiDatasetsV1 } from "../generated/contracts";
import { Button } from "../components";
export function CatalogueSearch({
  workspace,
  active,
  add,
  expanded = false,
  select,
}: {
  workspace: Workspace;
  active: boolean;
  add: (path: string) => void;
  expanded?: boolean;
  select?: (entry: ApiDatasetsV1["entries"][number]) => void;
}) {
  const [filter, setFilter] = useState("");
  const [result, setResult] = useState<{
    filter: string;
    page: ApiDatasetsV1;
  } | null>(null);
  const [message, setMessage] = useState("");
  const request = useRef<AbortController | null>(null);
  useEffect(() => {
    if (!active) request.current?.abort();
    return () => request.current?.abort();
  }, [active]);
  const search = async (cursor?: string): Promise<void> => {
    request.current?.abort();
    const current = new AbortController();
    request.current = current;
    setMessage("Searching catalogue…");
    const query = cursor && result ? result.filter : filter;
    try {
      const page = await workspace.read(
        "ApiDatasetsV1",
        "/api/v1/datasets",
        { filter: query, limit: "50", ...(cursor ? { cursor } : {}) },
        current.signal,
      );
      if (current.signal.aborted || request.current !== current) return;
      setResult({ filter: query, page });
      setMessage("");
    } catch (error) {
      if (!current.signal.aborted && request.current === current) {
        setResult(null);
        setMessage(
          error instanceof Error ? error.message : "Catalogue search failed.",
        );
      }
    }
  };
  return (
    <details className="catalogue-search" open={expanded || undefined}>
      <summary>Search the whole catalogue</summary>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void search();
        }}
      >
        <label>
          Catalogue path contains
          <input
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
        </label>
        <Button type="submit">Search catalogue</Button>
      </form>
      {message && <p role="status">{message}</p>}
      {result && (
        <>
          <p>
            {result.page.entries.length} results on this page ·{" "}
            {result.page.total} matching “{result.filter}”
          </p>
          <ul>
            {result.page.entries.map((entry) => (
              <li key={`${entry.workspace_id}:${entry.dataset_id}`}>
                <Button
                  onClick={() => (select ? select(entry) : add(entry.path))}
                >
                  {select ? entry.path : `Add ${entry.path} to view`}
                </Button>{" "}
                {entry.kind}
                {entry.origin === "external"
                  ? " · read-only foreign boundary"
                  : ""}
              </li>
            ))}
          </ul>
          {result.page.next_cursor && (
            <Button
              onClick={() => {
                if (result.page.next_cursor)
                  void search(result.page.next_cursor);
              }}
            >
              Next search results
            </Button>
          )}
        </>
      )}
    </details>
  );
}
