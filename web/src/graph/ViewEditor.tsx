import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { ApiViewsV1, GraphViewV1 } from "../generated/contracts";
import type { Workspace } from "../workspace";
import { queryFor } from "../workspace";
import { same } from "../api/validate";
import { Button } from "../components";
import { Icon } from "../Icons";
import type { Visual } from "./view";
import { createExport, exportDataUrl } from "./exports";
import { Menu } from "./Menu";

type Panel = "save" | "copy" | "open" | "json" | "svg" | "png" | null;
export function ViewEditor({
  workspace,
  visual,
  opened,
  onOpen,
  onMessage,
  titleHost,
  actionsHost,
}: {
  workspace: Workspace;
  visual: Visual;
  opened: GraphViewV1 | null;
  onOpen: (v: GraphViewV1) => Promise<void>;
  onMessage: (message: string) => void;
  titleHost: HTMLElement | null;
  actionsHost: HTMLElement | null;
}) {
  const [record, setRecord] = useState(opened);
  const [description, setDescription] = useState(opened?.description ?? "");
  const [editing, setEditing] = useState(false);
  const [descriptionDraft, setDescriptionDraft] = useState("");
  const [panel, setPanel] = useState<Panel>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (!panel) return;
    const previous = window.document.activeElement;
    const element = dialog.current;
    if (element && !element.open) element.showModal();
    (
      element?.querySelector<HTMLElement>('input:not([type="checkbox"])') ??
      element?.querySelector<HTMLElement>("button:not(:disabled)")
    )?.focus();
    return () => {
      element?.close();
      if (previous instanceof HTMLElement && previous.isConnected)
        previous.focus();
    };
  }, [panel]);
  const [name, setName] = useState("");
  const [page, setPage] = useState<ApiViewsV1 | null>(null);
  const [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const [labels, setLabels] = useState(true),
    [metadata, setMetadata] = useState(false);
  const [artifact, setArtifact] = useState<{
    url: string;
    preview: string;
    format: string;
  } | null>(null);
  const [id] = useState(() => crypto.randomUUID());
  const [copyId, setCopyId] = useState(() => crypto.randomUUID());
  const attempt = useRef<{ body: string; key: string } | null>(null);
  const lifetime = useRef(new AbortController());
  useEffect(() => {
    const request = new AbortController();
    lifetime.current = request;
    return () => request.abort();
  }, []);
  useEffect(
    () => () => {
      if (artifact) URL.revokeObjectURL(artifact.url);
    },
    [artifact],
  );
  const [previousOpened, setPreviousOpened] = useState(opened);
  if (previousOpened !== opened) {
    setPreviousOpened(opened);
    setRecord(opened);
    setDescription(opened?.description ?? "");
    setEditing(false);
  }
  const state = workspace.snapshot();
  const context = state.kind === "ready" ? state.value.context : null;
  const document: GraphViewV1 = {
    format_version: 1,
    id: record?.id ?? id,
    revision: record?.revision ?? 0,
    name: record?.name ?? "Untitled lineage",
    description,
    datasets: visual.nodes.map((n) => ({
      identity: n.identity,
      position: visual.positions[n.identity] ?? { x: 0, y: 0 },
    })),
    viewport: visual.viewport,
    colour: visual.colour,
    selector: {
      branch: state.selection.branch,
      fallback: context?.fallback_policy.slice(1) ?? [],
    },
  };
  const ordered = (value: GraphViewV1) => ({
    ...value,
    datasets: [...value.datasets].sort((a, b) =>
      a.identity.localeCompare(b.identity),
    ),
  });
  const dirty = record
    ? !same(ordered(document), ordered(record))
    : visual.nodes.length > 0;
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
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
        setError(
          e instanceof Error ? e.message : "The lineage operation failed.",
        );
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
  const save = async (copy = false) => {
    if (!context) return;
    const value = {
      ...document,
      ...(copy ? { id: copyId, revision: 0 } : {}),
      ...(!record || copy ? { name: name.trim() } : {}),
    };
    const body = JSON.stringify(value);
    if (attempt.current?.body !== body)
      attempt.current = { body, key: crypto.randomUUID() };
    const saved = await workspace.client.saveView(
      value,
      queryFor(state.selection),
      context,
      attempt.current.key,
      lifetime.current.signal,
    );
    setRecord(saved);
    setPanel(null);
    onMessage("Lineage saved.");
  };
  const show = (next: Panel) => {
    setPanel(next);
    setError("");
    setArtifact(null);
    if (next === "save" || next === "copy") {
      setName(
        next === "copy"
          ? `${record?.name ?? "Untitled lineage"} copy`.slice(0, 200)
          : "",
      );
      setCopyId(crypto.randomUUID());
    }
    if (next === "open") void run(() => list());
  };
  const heading = record && (
    <>
      <strong className="lineage-name" title={record.name}>
        {record.name}
        {dirty ? " *" : ""}
      </strong>
      <div className="lineage-description">
        {editing ? (
          <input
            aria-label="Lineage description"
            maxLength={2000}
            value={descriptionDraft}
            ref={(input) => input?.focus()}
            onChange={(e) => setDescriptionDraft(e.target.value)}
            onBlur={() => {
              setDescription(descriptionDraft.trim());
              setEditing(false);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                setDescription(descriptionDraft.trim());
                setEditing(false);
              }
              if (e.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <>
            <small title={description}>
              {description || "Add description"}
            </small>
            <Button
              aria-label="Edit lineage description"
              onClick={() => {
                setDescriptionDraft(description);
                setEditing(true);
              }}
            >
              <Icon name="edit" />
            </Button>
          </>
        )}
      </div>
    </>
  );
  const actions = (
    <div className="lineage-save">
      <Button
        disabled={!dirty || busy || !context || visual.nodes.length > 500}
        onClick={() => (record ? void run(() => save()) : show("save"))}
      >
        <Icon name="save" /> Save
      </Button>
      <Menu
        label="Lineage actions"
        items={[
          {
            label: "Save as",
            icon: <Icon name="edit" />,
            action: () => show("copy"),
          },
          {
            label: "Open lineage",
            icon: <Icon name="open" />,
            action: () => show("open"),
          },
          {
            label: "Export SVG",
            icon: <Icon name="export" />,
            action: () => show("svg"),
          },
          {
            label: "Export PNG",
            icon: <Icon name="export" />,
            action: () => show("png"),
          },
          {
            label: "Export JSON",
            icon: <Icon name="export" />,
            action: () => show("json"),
          },
        ]}
      />
      {error && !panel && (
        <div className="save-error" role="alert">
          {error}
        </div>
      )}
    </div>
  );
  return (
    <>
      {titleHost ? createPortal(heading, titleHost) : heading}
      {actionsHost ? createPortal(actions, actionsHost) : actions}
      {panel && (
        <div className="lineage-dialog-backdrop">
          <dialog
            ref={dialog}
            className="view-editor"
            onCancel={(event) => {
              event.preventDefault();
              if (!busy) setPanel(null);
            }}
            aria-modal="true"
            aria-label={
              panel === "open"
                ? "Open lineage"
                : panel === "save"
                  ? "Save lineage"
                  : panel === "copy"
                    ? "Save as"
                    : `Export ${panel.toUpperCase()}`
            }
            onKeyDown={(event) => {
              if (event.key === "Escape" && !busy) {
                setPanel(null);
                event.stopPropagation();
              }
              if (event.key === "Tab") {
                const controls = [
                  ...event.currentTarget.querySelectorAll<HTMLElement>(
                    "button:not(:disabled),input,a[href]",
                  ),
                ];
                const first = controls[0],
                  last = controls.at(-1);
                if (event.shiftKey && window.document.activeElement === first) {
                  event.preventDefault();
                  last?.focus();
                } else if (
                  !event.shiftKey &&
                  window.document.activeElement === last
                ) {
                  event.preventDefault();
                  first?.focus();
                }
              }
            }}
          >
            <header>
              <strong>
                {panel === "open"
                  ? "Open lineage"
                  : panel === "copy"
                    ? "Save as"
                    : panel === "save"
                      ? "Save lineage"
                      : `Export ${panel.toUpperCase()}`}
              </strong>
              <Button
                aria-label="Close lineage panel"
                disabled={busy}
                onClick={() => setPanel(null)}
              >
                <Icon name="close" />
              </Button>
            </header>
            {error && <p role="alert">{error}</p>}
            {(panel === "save" || panel === "copy") && (
              <>
                <label>
                  Lineage name
                  <input
                    ref={(input) => input?.focus()}
                    maxLength={200}
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    onKeyDown={(event) => {
                      if (
                        event.key === "Enter" &&
                        name.trim() &&
                        !busy &&
                        visual.nodes.length <= 500
                      )
                        void run(() => save(panel === "copy"));
                    }}
                  />
                </label>
                {visual.nodes.length > 500 && (
                  <p>
                    Save supports at most 500 datasets. Remove some from the
                    lineage before saving.
                  </p>
                )}
                <Button
                  disabled={
                    busy ||
                    !name.trim() ||
                    !context ||
                    visual.nodes.length > 500
                  }
                  onClick={() => void run(() => save(panel === "copy"))}
                >
                  Save lineage
                </Button>
              </>
            )}
            {panel === "open" && (
              <>
                {!page?.views.length && !busy && <p>No saved lineages.</p>}
                {page?.views.map((view) => (
                  <Button
                    key={view.id}
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        if (
                          dirty &&
                          !window.confirm(
                            "Discard unsaved changes and open this lineage?",
                          )
                        )
                          return;
                        const result = await workspace.client.read(
                          "GraphViewV1",
                          `/api/v1/views/${encodeURIComponent(view.id)}`,
                          {},
                          lifetime.current.signal,
                        );
                        await onOpen(result.data);
                        setPanel(null);
                      })
                    }
                  >
                    {view.name} · revision {view.revision}
                  </Button>
                ))}
                <div className="view-row">
                  <Button
                    disabled={busy}
                    onClick={() => void run(() => list())}
                  >
                    First page
                  </Button>
                  <Button
                    disabled={busy || !page?.next_cursor}
                    onClick={() =>
                      void run(() => list(page?.next_cursor ?? ""))
                    }
                  >
                    Next page
                  </Button>
                </div>
              </>
            )}
            {(panel === "json" || panel === "svg" || panel === "png") && (
              <>
                <label>
                  <input
                    type="checkbox"
                    checked={labels}
                    onChange={(e) => {
                      setLabels(e.target.checked);
                      setArtifact(null);
                    }}
                  />
                  Include dataset labels
                </label>
                <label>
                  <input
                    type="checkbox"
                    disabled={panel === "png"}
                    checked={panel !== "png" && metadata}
                    onChange={(e) => {
                      setMetadata(e.target.checked);
                      setArtifact(null);
                    }}
                  />
                  Include metadata
                </label>
                <Button
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      const blob = await createExport(panel, document, visual, {
                        labels,
                        metadata,
                      });
                      const preview =
                        panel === "json" ? "" : await exportDataUrl(blob);
                      if (!lifetime.current.signal.aborted)
                        setArtifact({
                          url: URL.createObjectURL(blob),
                          preview,
                          format: panel,
                        });
                    })
                  }
                >
                  Prepare export
                </Button>
                {artifact && (
                  <>
                    <a
                      href={artifact.url}
                      download={`lineage.${artifact.format}`}
                    >
                      Download {artifact.format.toUpperCase()}
                    </a>
                    {artifact.preview && (
                      <img
                        className="export-preview"
                        src={artifact.preview}
                        alt="Lineage export preview"
                      />
                    )}
                  </>
                )}
              </>
            )}
          </dialog>
        </div>
      )}
    </>
  );
}
