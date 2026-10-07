import { Icon } from "./Icons";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { ApiMetadataPageV1 } from "./generated/contracts";
import type { Workspace, WorkspaceState } from "./workspace";
import { Button } from "./components";

export function branchNames(state: WorkspaceState): string[] {
  const names = new Set([state.selection.branch]);
  if (state.kind === "ready")
    for (const entry of state.value.branches.entries) {
      if (
        entry &&
        typeof entry === "object" &&
        !Array.isArray(entry) &&
        "name" in entry &&
        typeof entry.name === "string" &&
        !("deleted" in entry && entry.deleted === true)
      )
        names.add(entry.name);
    }
  return [...names].sort((a, b) =>
    a === "master" ? -1 : b === "master" ? 1 : a.localeCompare(b),
  );
}
export function BranchControls({
  workspace,
  state,
  visible,
}: {
  workspace: Workspace;
  state: WorkspaceState;
  visible: readonly string[];
}) {
  const [open, setOpen] = useState(false),
    [search, setSearch] = useState("");
  const [editing, setEditing] = useState(false),
    [tail, setTail] = useState<string[]>([]);
  const [add, setAdd] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const picker = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    searchInput.current?.focus();
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !picker.current?.contains(event.target)
      )
        setOpen(false);
    };
    // Other widgets may stop bubbling pointer events; dismissal must still work.
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [open]);
  const drag = useRef<number | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const names = branchNames(state);
  const fingerprint =
    state.kind === "ready" ? state.value.context.fingerprint : "";
  const owner = state.kind === "ready" ? state.value.context.workspace : "";
  const scope = JSON.stringify(
    visible
      .filter((id) => id.startsWith(`dataset:${owner}:`))
      .map((id) => id.split(":")[2])
      .sort(),
  );
  const [scoped, setScoped] = useState<{ key: string; names: string[] } | null>(
    null,
  );
  const [scopeError, setScopeError] = useState("");
  const scopeKey = fingerprint + scope;
  useEffect(() => {
    if (!open || !fingerprint || scope === "[]") return;
    const request = new AbortController();
    void (async () => {
      try {
        const found = new Set<string>();
        let cursor: string | null = null;
        const seen = new Set<string>();
        do {
          const page: ApiMetadataPageV1 = await workspace.read(
            "ApiMetadataPageV1",
            "/api/v1/branches",
            { datasets: scope, limit: "200", ...(cursor ? { cursor } : {}) },
            request.signal,
          );
          for (const entry of page.entries)
            if (
              entry &&
              typeof entry === "object" &&
              "name" in entry &&
              typeof entry.name === "string" &&
              !("deleted" in entry && entry.deleted === true)
            )
              found.add(entry.name);
          cursor = page.next_cursor;
          if (cursor) {
            if (seen.has(cursor)) throw new Error("Repeated branch page");
            seen.add(cursor);
          }
        } while (cursor);
        if (!request.signal.aborted) {
          setScoped({ key: scopeKey, names: [...found].sort() });
          setScopeError("");
        }
      } catch {
        if (!request.signal.aborted)
          setScopeError("Could not load branches for the visible datasets.");
      }
    })();
    return () => request.abort();
  }, [open, fingerprint, scope, scopeKey, workspace]);
  const available =
    scope === "[]"
      ? names
      : scoped?.key === scopeKey
        ? [...new Set([state.selection.branch, ...scoped.names])]
        : [];

  const branch = state.selection.branch;
  const ready = state.kind === "ready";
  const pickerKey = (event: KeyboardEvent<HTMLElement>) => {
    if (open && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      picker.current
        ?.querySelector<HTMLButtonElement>('[aria-controls="branch-menu"]')
        ?.focus();
    }
  };
  useEffect(() => {
    if (editing) dialog.current?.showModal();
    else dialog.current?.close();
  }, [editing]);
  const close = (): void => {
    setEditing(false);
    trigger.current?.focus();
  };
  const reorder = (from: number, to: number): void => {
    if (to < 0 || to >= tail.length) return;
    setTail((old) => {
      const next = [...old];
      const [item] = next.splice(from, 1);
      if (item !== undefined) next.splice(to, 0, item);
      return next;
    });
  };
  return (
    <div className="branch-controls">
      <div className="branch-picker" ref={picker}>
        <Button
          aria-label={`Branch: ${branch}`}
          aria-expanded={open}
          aria-controls="branch-menu"
          onKeyDown={pickerKey}
          onClick={() => {
            setOpen(!open);
            setSearch("");
          }}
        >
          <Icon name="branch" /> Branch <strong>{branch}</strong>{" "}
          <Icon name="down" />
        </Button>
        {open && (
          <div
            id="branch-menu"
            role="dialog"
            aria-label="Branch selection"
            className="branch-menu"
          >
            <label>
              Search branches
              <input
                ref={searchInput}
                aria-label="Search branches"
                onKeyDown={pickerKey}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
            </label>
            <div
              className="branch-results"
              role="group"
              aria-label="Available branches"
            >
              {available
                .filter((n) => n.toLowerCase().includes(search.toLowerCase()))
                .map((name) => (
                  <button
                    key={name}
                    aria-pressed={branch === name}
                    disabled={!ready}
                    onKeyDown={pickerKey}
                    onClick={() => {
                      setOpen(false);
                      void workspace.select({
                        branch: name,
                        ...(state.selection.fallback
                          ? { fallback: state.selection.fallback }
                          : {}),
                      });
                    }}
                  >
                    <span aria-hidden="true">{name === branch ? "✓" : ""}</span>
                    {name}
                  </button>
                ))}
              {!available.some((n) =>
                n.toLowerCase().includes(search.toLowerCase()),
              ) && (
                <p>
                  {scopeError ||
                    (scope !== "[]" && scoped?.key !== scopeKey
                      ? "Loading branches…"
                      : "No matching branches")}
                </p>
              )}
            </div>
          </div>
        )}
      </div>
      <button
        ref={trigger}
        className="button icon-button"
        aria-label="Fallback branches"
        title="Fallback branches"
        disabled={!ready || !!state.selection.plan || !!state.selection.version}
        onClick={() => {
          setTail(
            state.kind === "ready"
              ? [...state.value.context.fallback_policy].filter(
                  (n) => n !== branch,
                )
              : [],
          );
          setAdd("");
          setEditing(true);
        }}
      >
        <Icon name="branch" />
      </button>
      <dialog
        ref={dialog}
        className="fallback-dialog"
        onCancel={close}
        aria-labelledby="fallback-title"
      >
        <header>
          <h2 id="fallback-title">
            <Icon name="branch" /> Fallback branches
          </h2>
          <Button
            className="fallback-close"
            aria-label="Close fallback branches"
            onClick={close}
          >
            <Icon name="close" />
          </Button>
        </header>
        <div className="fallback-body">
          <label className="fallback-add">
            <span className="sr-only">Add fallback branch</span>
            <span className="fallback-select">
              <Icon name="branch" />
              <select
                aria-label="Add fallback branch"
                value={add}
                onChange={(e) => {
                  const name = e.target.value;
                  if (name) {
                    setTail([...tail, name]);
                    setAdd("");
                  }
                }}
              >
                <option value="">Choose a branch…</option>
                {names
                  .filter((n) => n !== branch && !tail.includes(n))
                  .map((n) => (
                    <option key={n}>{n}</option>
                  ))}
              </select>
              <Icon name="down" />
            </span>
          </label>
          <p className="fallback-order">
            Try <strong>{branch}</strong> first, then these branches from top to
            bottom.
          </p>
          <ol className="fallback-list">
            {tail.map((name, i) => (
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
                  <Icon name="branch" /> {name}
                </span>
                <div className="fallback-row-actions">
                  <span className="fallback-grip" title="Drag to reorder">
                    <Icon name="grip" />
                  </span>
                  <Button
                    aria-label={`Move ${name} up`}
                    title="Move up"
                    disabled={i === 0}
                    onClick={() => reorder(i, i - 1)}
                  >
                    <Icon name="up" />
                  </Button>
                  <Button
                    aria-label={`Move ${name} down`}
                    title="Move down"
                    disabled={i === tail.length - 1}
                    onClick={() => reorder(i, i + 1)}
                  >
                    <Icon name="down" />
                  </Button>
                  <Button
                    aria-label={`Remove ${name} fallback`}
                    title="Remove fallback"
                    onClick={() => setTail(tail.filter((n) => n !== name))}
                  >
                    <Icon name="remove" />
                  </Button>
                </div>
              </li>
            ))}
          </ol>
          {!tail.length && (
            <p className="fallback-empty">
              No fallback branches. Only {branch} will be considered.
            </p>
          )}
          <p className="fallback-note">
            Applies to this view. Workspace defaults and independent named or
            external input policies stay unchanged.
          </p>
        </div>
        <footer>
          <Button
            onClick={() => {
              close();
              void workspace.select({ branch });
            }}
          >
            Use workspace defaults
          </Button>
          <Button
            className="button-primary"
            onClick={() => {
              close();
              void workspace.select({ branch, fallback: tail });
            }}
          >
            Save and close
          </Button>
          <Button onClick={close}>Cancel</Button>
        </footer>
      </dialog>
    </div>
  );
}
