import { useEffect, useId, useRef, useState } from "react";
import type { ApiMetadataPageV1 } from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { Icon } from "./Icons";

export function ScheduleBranchPicker({
  workspace,
  label,
  value,
  exclude = [],
  onChange,
}: {
  workspace: Workspace;
  label: string;
  value: string;
  exclude?: readonly string[];
  onChange: (name: string) => void;
}) {
  const [open, setOpen] = useState(false),
    [query, setQuery] = useState("");
  const [result, setResult] = useState<{
    names: string[];
    error: string;
  } | null>(null);
  const [revision, setRevision] = useState(0),
    [page, setPage] = useState(0);
  const root = useRef<HTMLDivElement>(null),
    input = useRef<HTMLInputElement>(null),
    button = useRef<HTMLButtonElement>(null);
  const id = useId();
  useEffect(() => {
    if (!open) return;
    input.current?.focus();
    const outside = (e: PointerEvent) => {
      if (e.target instanceof Node && !root.current?.contains(e.target))
        setOpen(false);
    };
    const escape = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        button.current?.focus();
      }
    };
    document.addEventListener("keydown", escape, true);
    document.addEventListener("pointerdown", outside, true);
    const abort = new AbortController();
    void (async () => {
      try {
        const names = new Set<string>(),
          seen = new Set<string>();
        let cursor: string | null = null,
          pages = 0;
        do {
          if (++pages > 1000)
            throw Error(
              "Too many branch pages. Branch selection is incomplete; retry after narrowing the workspace.",
            );
          const page: ApiMetadataPageV1 = await workspace.read(
            "ApiMetadataPageV1",
            "/api/v1/branches",
            { limit: "200", ...(cursor ? { cursor } : {}) },
            abort.signal,
          );
          for (const entry of page.entries)
            if (
              entry &&
              typeof entry === "object" &&
              "name" in entry &&
              typeof entry.name === "string" &&
              !("deleted" in entry && entry.deleted === true)
            )
              names.add(entry.name);
          cursor = page.next_cursor;
          if (cursor) {
            if (seen.has(cursor))
              throw Error(
                "Repeated branch page. Could not load all available branches.",
              );
            seen.add(cursor);
          }
        } while (cursor && !abort.signal.aborted);
        if (!abort.signal.aborted)
          setResult({ names: [...names].sort(), error: "" });
      } catch (e) {
        if (!abort.signal.aborted)
          setResult({
            names: [],
            error: e instanceof Error ? e.message : "Could not load branches",
          });
      }
    })();
    return () => {
      abort.abort();
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
    };
  }, [workspace, open, revision]);
  const matches =
    result?.names.filter(
      (name) =>
        !exclude.includes(name) &&
        name.toLowerCase().includes(query.toLowerCase()),
    ) ?? [];
  return (
    <div
      className="schedule-branch-picker branch-picker"
      role="group"
      aria-label={label}
      ref={root}
    >
      <span className="schedule-control-label">{label}</span>
      <button
        type="button"
        className="button"
        ref={button}
        aria-label={label}
        aria-expanded={open}
        aria-controls={id}
        onClick={() => {
          setOpen(!open);
          setQuery("");
          setPage(0);
          setResult(null);
        }}
      >
        <Icon name="branch" />
        <span>{value || "Choose a branch…"}</span>
        <Icon name="down" />
      </button>
      {open && (
        <div
          id={id}
          className="branch-menu"
          role="dialog"
          aria-label={`${label} selection`}
        >
          <label>
            Search branches
            <input
              ref={input}
              aria-label={`Search ${label.toLowerCase()}`}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setPage(0);
              }}
            />
          </label>
          <div
            className="branch-results"
            role="group"
            aria-label="Live available branches"
          >
            {matches.slice(page * 200, (page + 1) * 200).map((name) => (
              <button
                type="button"
                key={name}
                aria-pressed={name === value}
                onClick={() => {
                  onChange(name);
                  setOpen(false);
                  button.current?.focus();
                }}
              >
                <span aria-hidden="true">{value === name ? "✓" : ""}</span>
                {name}
              </button>
            ))}
            {matches.length > 200 && (
              <div className="schedule-branch-pages">
                <small>
                  Showing {page * 200 + 1}–
                  {Math.min((page + 1) * 200, matches.length)} of{" "}
                  {matches.length} branches
                </small>
                <Button
                  disabled={page === 0}
                  onClick={() => setPage((n) => n - 1)}
                >
                  Previous branches
                </Button>
                <Button
                  disabled={(page + 1) * 200 >= matches.length}
                  onClick={() => setPage((n) => n + 1)}
                >
                  Next branches
                </Button>
              </div>
            )}
            {!result ? (
              <p role="status">Loading all live branches…</p>
            ) : result.error ? (
              <>
                <p role="alert">{result.error}</p>
                <Button
                  onClick={() => {
                    setResult(null);
                    setRevision((n) => n + 1);
                  }}
                >
                  Retry branches
                </Button>
              </>
            ) : (
              !matches.length && <p>No matching branches</p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export function ScheduleBranchesEditor({
  workspace,
  branch,
  fallback,
  onBranch,
  onFallback,
}: {
  workspace: Workspace;
  branch: string;
  fallback: readonly string[];
  onBranch: (name: string) => void;
  onFallback: (names: readonly string[]) => void;
}) {
  const drag = useRef<number | null>(null);
  const reorder = (from: number, to: number) => {
    if (to < 0 || to >= fallback.length) return;
    const next = [...fallback],
      item = next.splice(from, 1)[0];
    if (item !== undefined) {
      next.splice(to, 0, item);
      onFallback(next);
    }
  };
  return (
    <section
      className="schedule-branches-editor"
      aria-label="Schedule branches"
    >
      <ScheduleBranchPicker
        workspace={workspace}
        label="Data branch"
        value={branch}
        onChange={(name) => {
          onBranch(name);
          onFallback(fallback.filter((n) => n !== name));
        }}
      />
      <h4>Fallback branches</h4>
      <p className="fallback-order">
        Read from the data branch first, then try fallbacks in order.
      </p>
      <div className="schedule-first-branch">
        <span className="schedule-branch-position">1</span>
        <Icon name="branch" />
        <span>{branch}</span>
        <small>Data branch</small>
      </div>
      <ol
        start={2}
        className="fallback-list"
        aria-label="Schedule fallback order"
      >
        {fallback.map((name, i) => (
          <li
            key={name}
            draggable
            onDragStart={() => {
              drag.current = i;
            }}
            onDragEnd={() => {
              drag.current = null;
            }}
            onDragOver={(e) => e.preventDefault()}
            onDrop={(e) => {
              e.preventDefault();
              if (drag.current !== null) reorder(drag.current, i);
              drag.current = null;
            }}
          >
            <span className="fallback-name">
              <Icon name="branch" />
              {name}
            </span>
            <div className="fallback-row-actions">
              <span className="fallback-grip" title="Drag to reorder">
                <Icon name="grip" />
              </span>
              <Button
                aria-label={`Move ${name} up`}
                disabled={i === 0}
                onClick={() => reorder(i, i - 1)}
              >
                <Icon name="up" />
              </Button>
              <Button
                aria-label={`Move ${name} down`}
                disabled={i === fallback.length - 1}
                onClick={() => reorder(i, i + 1)}
              >
                <Icon name="down" />
              </Button>
              <Button
                aria-label={`Remove ${name} fallback`}
                onClick={() => onFallback(fallback.filter((n) => n !== name))}
              >
                <Icon name="remove" />
              </Button>
            </div>
          </li>
        ))}
      </ol>
      <ScheduleBranchPicker
        workspace={workspace}
        label="Add fallback branch"
        value=""
        exclude={[branch, ...fallback]}
        onChange={(name) => onFallback([...fallback, name])}
      />
    </section>
  );
}
