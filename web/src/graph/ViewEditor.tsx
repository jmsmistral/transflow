import { useEffect, useRef, useState } from "react";
import type {
  ApiContextV1,
  ApiViewsV1,
  GraphViewV1,
  ViewNoteV1,
} from "../generated/contracts";
import type { Workspace } from "../workspace";
import { queryFor } from "../workspace";
import { same } from "../api/validate";
import { Button } from "../components";
import { groupsFor, type Visual } from "./view";
import { createExport, exportDataUrl } from "./exports";
import { label } from "./model";
export function ViewEditor({
  filter,
  onFilter,
  workspace,
  visual,
  opened,
  onOpen,
  onGroups,
  onNotes,
  onMessage,
}: {
  filter: string;
  onFilter: ((filter: string) => void) | undefined;
  workspace: Workspace;
  visual: Visual;
  opened: GraphViewV1 | null;
  onOpen: (v: GraphViewV1) => Promise<void>;
  onGroups: (groups: GraphViewV1["groups"]) => void;
  onNotes: (notes: GraphViewV1["annotations"]) => void;
  onMessage: (message: string) => void;
}) {
  const [open, setOpen] = useState(false),
    [tab, setTab] = useState("save");
  const [record, setRecord] = useState<GraphViewV1 | null>(opened);
  const [name, setName] = useState(opened?.name ?? "Untitled view"),
    [description, setDescription] = useState(opened?.description ?? "");
  const [mode, setMode] = useState<"branch" | "snapshot">(
    opened?.selector.mode ?? "branch",
  );
  const [page, setPage] = useState<ApiViewsV1 | null>(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const [grouping, setGrouping] = useState("selection"),
    [groupName, setGroupName] = useState("Group");
  const [noteText, setNoteText] = useState(""),
    [noteFormat, setNoteFormat] = useState<"text" | "markdown">("text"),
    [noteId, setNoteId] = useState<string | null>(null);
  const [labels, setLabels] = useState(true),
    [metadata, setMetadata] = useState(false),
    [notes, setNotes] = useState(false);
  const [format, setFormat] = useState<"json" | "svg" | "png">("json");
  const [artifact, setArtifact] = useState<{
    url: string;
    format: string;
    preview: string;
  } | null>(null);
  useEffect(
    () => () => {
      if (artifact) URL.revokeObjectURL(artifact.url);
    },
    [artifact],
  );
  const [id, setId] = useState(() => crypto.randomUUID());
  const attempt = useRef<{ body: string; key: string } | null>(null);
  const state = workspace.snapshot();
  const context: ApiContextV1 | null =
    state.kind === "ready" ? state.value.context : null;
  const lifetime = useRef(new AbortController());
  useEffect(() => {
    const request = new AbortController();
    lifetime.current = request;
    return () => request.abort();
  }, []);
  const [previousOpened, setPreviousOpened] = useState(opened);
  if (previousOpened !== opened) {
    setPreviousOpened(opened);
    setRecord(opened);
    setName(opened?.name ?? "Untitled view");
    setDescription(opened?.description ?? "");
    setMode(opened?.selector.mode ?? "branch");
  }
  const document: GraphViewV1 = {
    format_version: 1,
    id: record?.id ?? id,
    revision: record?.revision ?? 0,
    name,
    description,
    datasets: visual.nodes.map((n) => ({
      identity: n.identity,
      position: visual.positions[n.identity] ?? { x: 0, y: 0 },
    })),
    groups: visual.groups,
    annotations: visual.annotations,
    viewport: visual.viewport,
    colour: visual.colour,
    filters: { path: filter },
    selector: {
      branch: state.selection.branch,
      fallback: context?.fallback_policy.slice(1) ?? [],
      mode,
      source: mode === "snapshot" ? (context?.source ?? null) : null,
      graph: mode === "snapshot" ? (context?.graph ?? null) : null,
    },
  };
  const dirty = record
    ? !same(document, record)
    : !!(
        visual.nodes.length ||
        visual.annotations.length ||
        visual.groups.length ||
        description ||
        name !== "Untitled view"
      );
  useEffect(() => {
    if (!dirty) return;
    const warn = (e: BeforeUnloadEvent) => {
      e.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);
  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await work();
    } catch (e) {
      if (!lifetime.current.signal.aborted)
        setError(e instanceof Error ? e.message : "The view operation failed.");
    } finally {
      if (!lifetime.current.signal.aborted) setBusy(false);
    }
  };
  const list = async (after = "") => {
    const result = await workspace.client.read(
      "ApiViewsV1",
      "/api/v1/views",
      after ? { after } : {},
      lifetime.current.signal,
    );
    setPage(result.data);
  };
  const choose = async (viewId: string) => {
    if (
      dirty &&
      !window.confirm(
        "Discard unsaved view changes and open the saved branch/source context?",
      )
    )
      return;
    const result = await workspace.client.read(
      "GraphViewV1",
      `/api/v1/views/${encodeURIComponent(viewId)}`,
      {},
      lifetime.current.signal,
    );
    await onOpen(result.data);
    setOpen(false);
  };
  return (
    <>
      <Button
        onClick={() => {
          setOpen(!open);
        }}
        aria-expanded={open}
      >
        Views{dirty ? " •" : ""}
      </Button>
      {open && (
        <section className="view-editor" aria-label="Saved view editor">
          <header>
            <strong>
              {record?.name ?? "New view"}
              {dirty ? " · Unsaved changes" : " · Saved"}
            </strong>
            <Button
              aria-label="Close view editor"
              onClick={() => setOpen(false)}
            >
              ×
            </Button>
          </header>
          <nav aria-label="View actions">
            {["save", "open", "groups", "notes", "export"].map((t) => (
              <Button
                key={t}
                aria-pressed={tab === t}
                onClick={() => {
                  setTab(t);
                  if (t === "open") void run(() => list());
                }}
              >
                {t[0]?.toUpperCase()}
                {t.slice(1)}
              </Button>
            ))}
          </nav>
          {error && <p role="alert">{error}</p>}
          {tab === "save" && (
            <>
              <label>
                View name
                <input
                  maxLength={200}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                />
              </label>
              <label>
                Description
                <textarea
                  maxLength={2000}
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                />
              </label>
              <label>
                Source context
                <select
                  value={mode}
                  onChange={(e) =>
                    setMode(
                      e.target.value === "snapshot" ? "snapshot" : "branch",
                    )
                  }
                >
                  <option value="branch">Track branch tip</option>
                  <option
                    value="snapshot"
                    disabled={!context?.source || !context.graph}
                  >
                    Fixed source snapshot
                  </option>
                </select>
              </label>
              <p>
                {state.selection.branch} ·{" "}
                {mode === "snapshot"
                  ? `Fixed source ${context?.source ?? "unavailable"}`
                  : "Reopens the branch’s latest retained graph"}
                . Data heads are resolved at open time.
              </p>
              <label>
                Saved catalogue filter
                <input
                  value={filter}
                  maxLength={500}
                  onChange={(e) => onFilter?.(e.target.value)}
                />
              </label>
              <Button
                disabled={
                  busy || !name.trim() || !context || visual.nodes.length > 500
                }
                onClick={() =>
                  void run(async () => {
                    if (!context) return;
                    const body = JSON.stringify(document);
                    if (attempt.current?.body !== body)
                      attempt.current = { body, key: crypto.randomUUID() };
                    const saved = await workspace.client.saveView(
                      document,
                      queryFor(state.selection),
                      context,
                      attempt.current.key,
                      lifetime.current.signal,
                    );
                    setRecord(saved);
                    onMessage("View saved.");
                  })
                }
              >
                Save view
              </Button>
              <Button
                disabled={busy}
                onClick={() => {
                  setId(crypto.randomUUID());
                  setRecord(null);
                  setName(`${name} copy`.slice(0, 200));
                }}
              >
                Save as new view
              </Button>
              {visual.nodes.length > 500 && (
                <p>
                  Save supports at most 500 datasets. Remove some from the view
                  before saving.
                </p>
              )}
              {record && (
                <label>
                  Local view link
                  <input
                    readOnly
                    value={`${window.location.origin}${window.location.pathname}?view=${record.id}`}
                  />
                  <small>
                    Same coordinator only; opening still requires an
                    authenticated session.
                  </small>
                </label>
              )}
            </>
          )}
          {tab === "open" && (
            <>
              <p>
                Opening restores visual state and its branch/source selector.
              </p>
              {page?.views.map((v) => (
                <Button
                  key={v.id}
                  disabled={busy}
                  onClick={() => void run(() => choose(v.id))}
                >
                  {v.name} · revision {v.revision}
                </Button>
              ))}
              {page?.views.length === 0 && <p>No saved views yet.</p>}
              <div>
                <Button disabled={busy} onClick={() => void run(() => list())}>
                  First page
                </Button>
                <Button
                  disabled={busy || !page?.next_cursor}
                  onClick={() => void run(() => list(page?.next_cursor ?? ""))}
                >
                  Next page
                </Button>
              </div>
            </>
          )}
          {tab === "groups" && (
            <>
              <label>
                Group by
                <select
                  value={grouping}
                  onChange={(e) => setGrouping(e.target.value)}
                >
                  <option value="selection">Selected datasets</option>
                  <option value="path">Logical path directory</option>
                  <option value="source">Source directory</option>
                  <option value="provider">External provider</option>
                  <option value="colour">Current colour category</option>
                </select>
              </label>
              {grouping === "selection" && (
                <label>
                  Group name
                  <input
                    value={groupName}
                    maxLength={200}
                    onChange={(e) => setGroupName(e.target.value)}
                  />
                </label>
              )}
              <Button
                disabled={
                  busy ||
                  !visual.nodes.length ||
                  (grouping === "selection" && !visual.selected.length)
                }
                onClick={() =>
                  void run(async () => {
                    const members =
                      grouping === "selection"
                        ? visual.nodes.filter((n) =>
                            visual.selected.includes(n.identity),
                          )
                        : visual.nodes;
                    const source = new Map<string, string>();
                    if (grouping === "source")
                      for (const n of members) {
                        const [, origin, dataset] = n.identity.split(":");
                        if (!dataset || !origin) continue;
                        const detail = await workspace.read(
                          "ApiDatasetV1",
                          `/api/v1/datasets/${encodeURIComponent(dataset)}`,
                          { origin_workspace: origin },
                          lifetime.current.signal,
                        );
                        const producer = detail.producer;
                        if (
                          producer &&
                          typeof producer === "object" &&
                          !Array.isArray(producer)
                        ) {
                          const path: unknown = Reflect.get(producer, "path");
                          if (typeof path === "string")
                            source.set(
                              n.identity,
                              path.split("/").slice(0, -1).join("/") ||
                                "Source root",
                            );
                        }
                      }
                    const groups = groupsFor(members, visual.positions, (n) =>
                      grouping === "selection"
                        ? groupName
                        : grouping === "path"
                          ? label(n).split("/").slice(0, -1).join("/") || "Root"
                          : grouping === "source"
                            ? (source.get(n.identity) ?? "No retained source")
                            : grouping === "provider"
                              ? n.external
                                ? `Provider ${n.identity.split(":")[1]}`
                                : "Local workspace"
                              : visual.colour === "resource"
                                ? n.resource_type
                                : n.publication,
                    );
                    const keep =
                      grouping === "selection"
                        ? visual.groups
                            .map((g) => ({
                              ...g,
                              members: g.members.filter(
                                (id) => !visual.selected.includes(id),
                              ),
                            }))
                            .filter((g) => g.members.length)
                        : [];
                    if (keep.length + groups.length > 100)
                      throw new Error("A view supports at most 100 groups.");
                    onGroups([...keep, ...groups]);
                  })
                }
              >
                Create groups
              </Button>
              <p>
                Groups are visual only. Collapsed status counts published,
                not-built and unknown members separately.
              </p>
              {visual.groups.map((g) => (
                <div className="view-list-row" key={g.id}>
                  <span>
                    {g.name} ({g.members.length})
                  </span>
                  <Button
                    onClick={() =>
                      onGroups(
                        visual.groups.map((v) =>
                          v.id === g.id ? { ...v, collapsed: !v.collapsed } : v,
                        ),
                      )
                    }
                  >
                    {g.collapsed ? "Expand" : "Collapse"}
                  </Button>
                  <Button
                    onClick={() =>
                      onGroups(visual.groups.filter((v) => v.id !== g.id))
                    }
                  >
                    Ungroup
                  </Button>
                </div>
              ))}
            </>
          )}
          {tab === "notes" && (
            <>
              <label>
                Annotation
                <textarea
                  maxLength={8000}
                  value={noteText}
                  onChange={(e) => setNoteText(e.target.value)}
                />
              </label>
              <label>
                Note format
                <select
                  value={noteFormat}
                  onChange={(e) =>
                    setNoteFormat(
                      e.target.value === "markdown" ? "markdown" : "text",
                    )
                  }
                >
                  <option value="text">Plain text</option>
                  <option value="markdown">Restricted Markdown</option>
                </select>
              </label>
              <p>
                Markdown supports **bold**, *emphasis* and `code`. HTML, images
                and links render as text; no remote resources load.
              </p>
              <Button
                disabled={
                  !noteText.trim() ||
                  (!noteId && visual.annotations.length >= 100)
                }
                onClick={() => {
                  const old = visual.annotations.find((n) => n.id === noteId);
                  const note: ViewNoteV1 = {
                    id: noteId ?? crypto.randomUUID(),
                    text: noteText,
                    format: noteFormat,
                    position: old?.position ?? {
                      x: (80 - visual.viewport.x) / visual.viewport.zoom,
                      y: (160 - visual.viewport.y) / visual.viewport.zoom,
                    },
                  };
                  onNotes([
                    ...visual.annotations.filter((n) => n.id !== note.id),
                    note,
                  ]);
                  setNoteText("");
                  setNoteId(null);
                }}
              >
                {noteId ? "Update note" : "Add note"}
              </Button>
              {visual.annotations.map((n) => (
                <div className="view-list-row" key={n.id}>
                  <span>{n.text.slice(0, 80)}</span>
                  <Button
                    onClick={() => {
                      setNoteId(n.id);
                      setNoteText(n.text);
                      setNoteFormat(n.format);
                    }}
                  >
                    Edit note
                  </Button>
                  <Button
                    onClick={() => {
                      onNotes(visual.annotations.filter((v) => v.id !== n.id));
                      if (noteId === n.id) {
                        setNoteId(null);
                        setNoteText("");
                      }
                    }}
                  >
                    Remove note
                  </Button>
                </div>
              ))}
            </>
          )}
          {tab === "export" && (
            <>
              <label>
                Export format
                <select
                  value={format}
                  onChange={(e) =>
                    setFormat(
                      e.target.value === "svg"
                        ? "svg"
                        : e.target.value === "png"
                          ? "png"
                          : "json",
                    )
                  }
                >
                  <option>json</option>
                  <option>svg</option>
                  <option>png</option>
                </select>
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={labels}
                  onChange={(e) => setLabels(e.target.checked)}
                />
                Include dataset and group labels
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={metadata}
                  onChange={(e) => setMetadata(e.target.checked)}
                />
                Include view and dataset metadata (JSON/SVG)
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={notes}
                  onChange={(e) => setNotes(e.target.checked)}
                />
                Include annotations
              </label>
              <p>
                Exports contain this view only. PNG is scaled to at most 4096
                pixels per side and contains rendered content only.
              </p>
              <Button
                disabled={busy || !visual.nodes.length}
                onClick={() =>
                  void run(async () => {
                    const blob = await createExport(format, document, visual, {
                      labels,
                      metadata,
                      notes,
                    });
                    const preview =
                      format === "json" ? "" : await exportDataUrl(blob);
                    if (!lifetime.current.signal.aborted)
                      setArtifact({
                        url: URL.createObjectURL(blob),
                        format,
                        preview,
                      });
                  })
                }
              >
                Prepare export
              </Button>
              {artifact && (
                <div className="view-export-result">
                  <a
                    href={artifact.url}
                    download={`transflow-view.${artifact.format}`}
                  >
                    Download prepared {artifact.format.toUpperCase()}
                  </a>
                  <p>
                    This file uses the options selected when prepared. Prepare
                    again after changes.
                  </p>
                  {artifact.format !== "json" && (
                    <img src={artifact.preview} alt="Prepared view export" />
                  )}
                </div>
              )}
            </>
          )}
        </section>
      )}
    </>
  );
}
