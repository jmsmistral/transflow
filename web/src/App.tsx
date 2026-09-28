import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { Button, Status } from "./components";
import { ResizeHandle } from "./panels";
import { GraphExplorer } from "./graph/Graph";
import { Workspace } from "./workspace";
import type { ExecutionJsonV1 } from "./generated/contracts";

const tabs = [
  "Preview",
  "SQL scratchpad",
  "History",
  "Code",
  "Build timeline",
  "Data health",
] as const;
const modes = [
  "Catalogue",
  "Properties",
  "Build planner",
  "Schedules",
  "Health",
] as const;
function text(value: ExecutionJsonV1, field: string): string | undefined {
  if (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    field in value
  ) {
    const item: unknown = Reflect.get(value, field);
    if (typeof item === "string") return item;
  }
  return undefined;
}
function moveTab(
  event: KeyboardEvent,
  names: readonly string[],
  index: number,
  set: (name: string) => void,
): void {
  let next = index;
  if (event.key === "ArrowRight") next = (index + 1) % names.length;
  else if (event.key === "ArrowLeft")
    next = (index + names.length - 1) % names.length;
  else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = names.length - 1;
  else return;
  event.preventDefault();
  const name = names[next];
  if (name) {
    set(name);
    const buttons =
      event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
        "[role=tab]",
      );
    buttons?.[next]?.focus();
  }
}
export function App({ workspace: supplied }: { workspace?: Workspace }) {
  const [workspace] = useState(() => supplied ?? new Workspace());
  const state = useSyncExternalStore(workspace.subscribe, workspace.snapshot);
  const [dark, setDark] = useState(false);
  const [right, setRight] = useState(30),
    [bottom, setBottom] = useState(32);
  const [inspectorOpen, setInspectorOpen] = useState(true),
    [bottomOpen, setBottomOpen] = useState(true);
  const [mode, setMode] = useState<string>("Catalogue"),
    [tab, setTab] = useState<string>("Preview");
  const [branch, setBranch] = useState("main");
  const ready = state.kind === "ready" ? state.value : null;
  const dataset = ready?.dataset;
  const selectedButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (state.kind === "ready" && document.activeElement === document.body)
      selectedButton.current?.focus();
  }, [state]);
  const requested = state.selection.branch;
  const title =
    dataset?.path ??
    (state.selection.dataset ? "Selected dataset" : "Workspace");
  const status =
    state.kind === "ready"
      ? "Connected"
      : state.kind === "failed"
        ? "Request failed"
        : state.kind === "disconnected"
          ? "Not connected"
          : state.kind === "connecting"
            ? "Connecting"
            : "Loading context";
  const panelStyle = {
    "--inspector": `${right}%`,
    "--bottom": `${bottom}%`,
  } as CSSProperties;
  return (
    <div className={`app ${dark ? "theme-dark" : ""}`}>
      <a className="skip-link" href="#workspace">
        Skip to workspace
      </a>
      <header className="topbar">
        <a className="brand" href="#workspace" aria-label="Transflow workspace">
          <span aria-hidden="true">◈</span> transflow
        </a>
        <nav aria-label="Workspace breadcrumb" className="breadcrumb">
          <span>Workspace</span>
          <span aria-hidden="true">/</span>
          <strong>{title === "Workspace" ? "Overview" : title}</strong>
        </nav>
        <div className="top-actions">
          <Status>{status}</Status>
          <Button aria-pressed={dark} onClick={() => setDark(!dark)}>
            Dark theme
          </Button>
        </div>
      </header>
      <section className="contextbar" aria-label="Workspace context">
        <div>
          <span className="eyebrow">View</span>
          <strong>Unsaved workspace view</strong>
          <span className="muted">View saving comes later</span>
        </div>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void workspace.select({ branch: branch.trim() });
          }}
        >
          <label htmlFor="branch">Source / data branch</label>
          <div className="input-group">
            <input
              id="branch"
              list="branches"
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              maxLength={256}
              disabled={state.kind === "connecting"}
            />
            <Button
              type="submit"
              disabled={!branch.trim() || state.kind === "connecting"}
            >
              Apply branch
            </Button>
          </div>
          <datalist id="branches">
            {ready?.branches.entries.map((entry) => {
              const name = text(entry, "name");
              return name ? <option key={name} value={name} /> : null;
            })}
          </datalist>
        </form>
        <div>
          <span className="eyebrow">Requested branch</span>
          <strong>{requested}</strong>
          <span className="muted">
            {ready
              ? `Fallback: ${ready.context.fallback_policy.join(" → ")}`
              : "Fallback policy not loaded"}
          </span>
        </div>
        <div>
          <span className="eyebrow">Build output branch</span>
          <strong>{requested}</strong>
          <span className="muted">Build controls are not available yet</span>
        </div>
      </section>
      <main
        id="workspace"
        tabIndex={-1}
        className={`workspace ${inspectorOpen ? "" : "inspector-closed"} ${bottomOpen ? "" : "bottom-closed"}`}
        style={panelStyle}
      >
        <div className="work-area">
          <section className="graph-region" aria-labelledby="workspace-title">
            <div className="section-heading">
              <div>
                <p className="eyebrow">Workspace / lineage</p>
                <h1 id="workspace-title">
                  {state.kind === "disconnected"
                    ? "No workspace connected"
                    : "Dataset workspace"}
                </h1>
              </div>
              <div className="toolbar">
                <Button
                  aria-expanded={inspectorOpen}
                  aria-controls="inspector"
                  onClick={() => setInspectorOpen(!inspectorOpen)}
                >
                  Inspector
                </Button>
                <Button
                  aria-expanded={bottomOpen}
                  aria-controls="bottom-panel"
                  onClick={() => setBottomOpen(!bottomOpen)}
                >
                  Bottom panel
                </Button>
              </div>
            </div>
            <div className="canvas">
              <GraphExplorer workspace={workspace} dark={dark} />
              {!ready && (
                <div className="canvas-message" role="status">
                  <h2>
                    {state.kind === "failed"
                      ? "This context could not be loaded"
                      : state.kind === "disconnected"
                        ? "Connect to a local coordinator"
                        : "Loading workspace context…"}
                  </h2>
                  <p>
                    {state.kind === "failed" || state.kind === "disconnected"
                      ? state.message
                      : "Waiting for verified workspace metadata."}
                  </p>
                  <p className="muted">No dataset information loaded</p>
                  {(state.kind === "failed" ||
                    state.kind === "disconnected") && (
                    <Button onClick={workspace.reconnect}>
                      Reconnect / refresh
                    </Button>
                  )}
                </div>
              )}
            </div>
            <footer className="canvas-footer">
              <span>
                {ready
                  ? `${ready.datasets.entries.length} of ${ready.datasets.total} datasets on this page`
                  : "Context unavailable"}
              </span>
              <span>Read-only browsing</span>
            </footer>
          </section>
          {inspectorOpen && (
            <>
              <ResizeHandle
                axis="horizontal"
                value={right}
                change={setRight}
                controls="inspector"
              />
              <aside
                id="inspector"
                className="inspector"
                aria-label="Right inspector"
              >
                <label className="inspector-mode">
                  Inspector mode
                  <select
                    value={mode}
                    onChange={(event) => setMode(event.target.value)}
                  >
                    {modes.map((name) => (
                      <option key={name}>{name}</option>
                    ))}
                  </select>
                </label>
                {mode === "Catalogue" ? (
                  <>
                    <h2>Datasets</h2>
                    {ready ? (
                      <>
                        <ul className="dataset-list">
                          {ready.datasets.entries.map((entry) => (
                            <li
                              key={`${entry.workspace_id}/${entry.dataset_id}`}
                            >
                              <button
                                ref={
                                  entry.dataset_id ===
                                    state.selection.dataset &&
                                  entry.workspace_id === state.selection.origin
                                    ? selectedButton
                                    : null
                                }
                                aria-pressed={
                                  entry.dataset_id ===
                                    state.selection.dataset &&
                                  entry.workspace_id === state.selection.origin
                                }
                                onClick={() => {
                                  void workspace.select({
                                    branch: requested,
                                    dataset: entry.dataset_id,
                                    origin: entry.workspace_id,
                                  });
                                }}
                              >
                                <span>{entry.path}</span>
                                <small>
                                  {entry.origin} · {entry.kind}
                                </small>
                              </button>
                            </li>
                          ))}
                        </ul>
                        {ready.datasets.entries.length === 0 && (
                          <p className="muted">No datasets to display.</p>
                        )}
                        <div className="toolbar">
                          <Button
                            disabled={!state.selection.cursor}
                            onClick={() => {
                              void workspace.select({ branch: requested });
                            }}
                          >
                            First page
                          </Button>
                          <Button
                            disabled={!ready.datasets.next_cursor}
                            onClick={() => {
                              if (ready.datasets.next_cursor)
                                void workspace.select({
                                  branch: requested,
                                  cursor: ready.datasets.next_cursor,
                                });
                            }}
                          >
                            Next page
                          </Button>
                        </div>
                      </>
                    ) : (
                      <p className="muted">{status}</p>
                    )}
                  </>
                ) : mode === "Properties" ? (
                  <h2>Selected context</h2>
                ) : (
                  <>
                    <h2>{mode}</h2>
                    <p className="muted">
                      This inspector is not available yet.
                    </p>
                  </>
                )}
                {(mode === "Catalogue" || mode === "Properties") && (
                  <section
                    className="selection-details"
                    aria-label="Selected dataset context"
                  >
                    <h3>{dataset?.path ?? "No dataset selected"}</h3>
                    {dataset && ready && (
                      <>
                        <dl>
                          <dt>Requested branch</dt>
                          <dd>{requested}</dd>
                          <dt>Resolved head branch</dt>
                          <dd>
                            {text(dataset.head, "resolved_branch") ??
                              (state.selection.version
                                ? "Not queried for an exact version"
                                : "Unavailable")}
                          </dd>
                          <dt>
                            {state.selection.version
                              ? "Producing source capture"
                              : "Retained definition source"}
                          </dt>
                          <dd>
                            {ready.context.source ??
                              (dataset.origin === "external"
                                ? "Foreign source metadata only"
                                : "Unavailable")}
                          </dd>
                          <dt>Exact version</dt>
                          <dd>
                            {state.selection.version ??
                              "Not pinned — browsing retained metadata"}
                          </dd>
                          <dt>Published head</dt>
                          <dd>
                            {text(dataset.head, "version") ??
                              (state.selection.version
                                ? "Not queried in historical context"
                                : dataset.origin === "external"
                                  ? "Foreign head not loaded"
                                  : "No published head")}
                          </dd>
                        </dl>
                        <label>
                          Version context
                          <select
                            value={state.selection.version ?? ""}
                            onChange={(event) => {
                              void workspace.select({
                                branch: requested,
                                dataset: dataset.dataset_id,
                                origin: dataset.workspace_id,
                                ...(event.target.value
                                  ? { version: event.target.value }
                                  : {}),
                              });
                            }}
                          >
                            <option value="">
                              Current retained definition
                            </option>
                            {ready.versions?.entries.map((version) => (
                              <option
                                key={version.version}
                                value={version.version}
                              >
                                {version.version}
                              </option>
                            ))}
                          </select>
                        </label>
                        {ready.versions?.next_cursor && (
                          <p className="muted">
                            More versions exist; the full history inspector is
                            coming later.
                          </p>
                        )}
                        <p className="muted">
                          Metadata availability does not verify dataset bytes or
                          freshness.
                        </p>
                      </>
                    )}
                  </section>
                )}
              </aside>
            </>
          )}
        </div>
        {bottomOpen && (
          <>
            <ResizeHandle
              axis="vertical"
              value={bottom}
              change={setBottom}
              controls="bottom-panel"
            />
            <section
              id="bottom-panel"
              className="bottom-panel"
              aria-label="Dataset inspectors"
            >
              <div role="tablist" aria-label="Dataset inspector tabs">
                {tabs.map((name, index) => (
                  <button
                    key={name}
                    id={`tab-${index}`}
                    role="tab"
                    aria-controls="inspector-content"
                    aria-selected={tab === name}
                    tabIndex={tab === name ? 0 : -1}
                    onClick={() => setTab(name)}
                    onKeyDown={(event) => moveTab(event, tabs, index, setTab)}
                  >
                    {name}
                  </button>
                ))}
              </div>
              <div
                id="inspector-content"
                role="tabpanel"
                aria-labelledby={`tab-${tabs.findIndex((name) => name === tab)}`}
                tabIndex={0}
              >
                <p className="eyebrow">
                  {title} · {requested}
                </p>
                <h2>{tab}</h2>
                <p>
                  {!ready
                    ? "Content is unavailable until this context is loaded."
                    : !dataset
                      ? "Select a dataset to establish inspector context."
                      : `${tab} content will be available in a later inspector task.`}
                </p>
                {dataset && (
                  <p className="muted">
                    This panel is bound to the selected context. No rows or
                    source code have been fetched.
                  </p>
                )}
              </div>
            </section>
          </>
        )}
      </main>
      <footer className="app-footer">
        <span>
          Local workspace · {ready?.context.workspace ?? "not connected"}
        </span>
        <span>Transflow · Development interface</span>
      </footer>
    </div>
  );
}
