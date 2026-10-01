import { useCallback, useState, useSyncExternalStore } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { Button, InspectorNotice, Status } from "./components";
import { Icon, type IconName } from "./Icons";
import { ResizeHandle } from "./panels";
import { BranchControls } from "./BranchControls";
import { CatalogueSearch } from "./graph/Search";
import { GraphExplorer } from "./graph/Graph";
import { Workspace, visualContext } from "./workspace";
import type { ApiLineageNodeV1 } from "./generated/contracts";
import type { Snapshot, WorkspaceState } from "./workspace";
import { CodeInspector } from "./CodeInspector";
import { HistoryInspector } from "./HistoryInspector";
import { TimelineInspector } from "./BuildInspectors";
import { PreviewInspector, PropertiesInspector } from "./inspectors";

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
  const [focusRequest, setFocusRequest] = useState<{
    path: string;
    revision: number;
  } | null>(null);
  const [catalogueFilter, setCatalogueFilter] = useState("");
  const [titleHost, setTitleHost] = useState<HTMLDivElement | null>(null);
  const [actionsHost, setActionsHost] = useState<HTMLDivElement | null>(null);
  const [cataloguePaths, setCataloguePaths] = useState<readonly string[]>([]);
  const [visible, setVisible] = useState<readonly string[]>([]);
  const [graphSelection, setGraphSelection] = useState<{
    context: string;
    visual: string;
    nodes: readonly ApiLineageNodeV1[];
  }>({ context: "", visual: "", nodes: [] });
  const handleSelection = useCallback(
    (nodes: readonly ApiLineageNodeV1[], context: string) => {
      const snapshot = workspace.snapshot();
      setGraphSelection({
        context,
        nodes,
        visual:
          snapshot.kind === "ready" &&
          snapshot.value.context.fingerprint === context
            ? visualContext(snapshot.value.context)
            : "",
      });
      if (nodes.length === 1) {
        const [, origin, id] = nodes[0]?.identity.split(":") ?? [];
        const current = workspace.snapshot();
        if (
          id &&
          origin &&
          current.kind === "ready" &&
          current.value.context.fingerprint === context &&
          (current.selection.dataset !== id ||
            current.selection.origin !== origin)
        )
          void workspace.select({
            branch: current.selection.branch,
            dataset: id,
            origin,
            ...(current.selection.fallback
              ? { fallback: current.selection.fallback }
              : {}),
          });
      }
    },
    [workspace],
  );
  const [dark, setDark] = useState(false);
  const [right, setRight] = useState(26),
    [bottom, setBottom] = useState(32);
  const [inspectorOpen, setInspectorOpen] = useState(false),
    [bottomOpen, setBottomOpen] = useState(false);
  const [mode, setMode] = useState<string>("Catalogue"),
    [tab, setTab] = useState<string>("Preview");
  const ready = state.kind === "ready" ? state.value : null;
  const selectedNodes =
    ready && graphSelection.visual === visualContext(ready.context)
      ? graphSelection.nodes
      : [];
  const [pinnedVersion, setPinnedVersion] = useState<{
    key: string;
    version: NonNullable<Snapshot["versions"]>["entries"][number];
  } | null>(null);
  const inspectionKey = `${ready ? visualContext(ready.context) : ""}:${selectedNodes[0]?.identity}`;
  const pinKey = JSON.stringify([
    ready?.context.workspace,
    state.selection.branch,
    ready?.context.fallback_policy,
    selectedNodes[0]?.identity,
  ]);
  const pin = pinnedVersion?.key === pinKey ? pinnedVersion.version : null;
  const inspectorState: WorkspaceState =
    state.kind === "ready" && pin
      ? {
          ...state,
          selection: { ...state.selection, version: pin.version },
          value: {
            ...state.value,
            versions: {
              entries: [
                pin,
                ...(state.value.versions?.entries.filter(
                  (v) => v.version !== pin.version,
                ) ?? []),
              ],
              next_cursor: state.value.versions?.next_cursor ?? null,
            },
          },
        }
      : state;
  const dataset = ready?.dataset;
  const inspected =
    selectedNodes.length === 1 &&
    selectedNodes[0]?.identity ===
      `dataset:${dataset?.workspace_id}:${dataset?.dataset_id}`;
  const requested = state.selection.branch;
  const title =
    dataset?.path ??
    (state.selection.dataset ? "Selected dataset" : "Workspace");
  const status =
    state.kind === "ready"
      ? state.refreshing
        ? "Refreshing…"
        : "Connected"
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
        <div className="lineage-heading" ref={setTitleHost} />
        <div className="top-actions">
          <Button aria-pressed={dark} onClick={() => setDark(!dark)}>
            Dark theme
          </Button>
          <BranchControls
            workspace={workspace}
            state={state}
            visible={visible}
          />
          <div ref={setActionsHost} />
          <Status>{status}</Status>
        </div>
      </header>
      <main
        id="workspace"
        tabIndex={-1}
        className={`workspace ${inspectorOpen ? "" : "inspector-closed"} ${bottomOpen ? "" : "bottom-closed"}`}
        style={panelStyle}
      >
        <div className="work-area">
          <section className="graph-region" aria-labelledby="workspace-title">
            <h1 id="workspace-title" className="sr-only">
              Dataset lineage
            </h1>
            <div className="canvas">
              <GraphExplorer
                workspace={workspace}
                dark={dark}
                onVisible={setVisible}
                onSelection={handleSelection}
                focusRequest={focusRequest}
                cataloguePaths={cataloguePaths}
                titleHost={titleHost}
                actionsHost={actionsHost}
              />
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
          </section>
          <nav className="inspector-rail" aria-label="Inspector tools">
            {modes.map((name, i) => (
              <button
                key={name}
                title={name}
                aria-label={name}
                aria-pressed={inspectorOpen && mode === name}
                onClick={() => {
                  setMode(name);
                  setInspectorOpen(true);
                }}
              >
                <Icon
                  name={
                    (
                      [
                        "search",
                        "list",
                        "build",
                        "calendar",
                        "health",
                      ] satisfies IconName[]
                    )[i] ?? "search"
                  }
                />
              </button>
            ))}
            <button
              title={
                inspectorOpen ? "Collapse right panel" : "Expand right panel"
              }
              aria-label={
                inspectorOpen ? "Collapse right panel" : "Expand right panel"
              }
              aria-expanded={inspectorOpen}
              aria-controls="inspector"
              onClick={() => setInspectorOpen(!inspectorOpen)}
            >
              <Icon name={inspectorOpen ? "collapse" : "reveal"} />
            </button>
          </nav>
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
                className={`inspector ${mode === "Catalogue" ? "catalogue-panel" : ""}`}
                aria-label="Right inspector"
              >
                <header className="inspector-header">
                  <h2>{mode}</h2>
                </header>
                {mode === "Catalogue" ? (
                  <>
                    <CatalogueSearch
                      query={catalogueFilter}
                      onQuery={setCatalogueFilter}
                      workspace={workspace}
                      active={!!ready}
                      visible={visible}
                      onPage={setCataloguePaths}
                      select={(entry) => {
                        setFocusRequest((old) => ({
                          path: entry.path,
                          revision: (old?.revision ?? 0) + 1,
                        }));
                        void workspace.select({
                          branch: requested,
                          dataset: entry.dataset_id,
                          origin: entry.workspace_id,
                          ...(state.selection.fallback
                            ? { fallback: state.selection.fallback }
                            : {}),
                        });
                      }}
                    />
                  </>
                ) : mode === "Properties" ? (
                  selectedNodes.length === 1 && !inspected ? (
                    <div className="execution-empty" role="status">
                      Loading dataset information…
                    </div>
                  ) : (
                    <PropertiesInspector
                      state={inspectorState}
                      selected={selectedNodes}
                    />
                  )
                ) : (
                  <>
                    <h2>{mode}</h2>
                    <p className="muted">
                      This inspector is not available yet.
                    </p>
                  </>
                )}
              </aside>
            </>
          )}
        </div>
        <>
          {bottomOpen && (
            <ResizeHandle
              axis="vertical"
              value={bottom}
              change={setBottom}
              controls="bottom-panel"
            />
          )}
          <section
            id="bottom-panel"
            className={`bottom-panel ${bottomOpen ? "" : "collapsed"}`}
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
                  onClick={() => {
                    setTab(name);
                    setBottomOpen(true);
                  }}
                  onKeyDown={(event) =>
                    moveTab(event, tabs, index, (name) => {
                      setTab(name);
                      setBottomOpen(true);
                    })
                  }
                >
                  {name}
                </button>
              ))}
              <button
                className="panel-toggle"
                aria-label={
                  bottomOpen ? "Collapse bottom panel" : "Expand bottom panel"
                }
                aria-expanded={bottomOpen}
                aria-controls="inspector-content"
                onClick={() => setBottomOpen(!bottomOpen)}
              >
                <Icon name={bottomOpen ? "down" : "up"} />
              </button>
            </div>
            <div
              hidden={!bottomOpen}
              id="inspector-content"
              role="tabpanel"
              aria-labelledby={`tab-${tabs.findIndex((name) => name === tab)}`}
              tabIndex={0}
            >
              {selectedNodes.length !== 1 ? (
                <InspectorNotice />
              ) : !inspected ? (
                <div className="execution-empty" role="status">
                  Loading dataset information…
                </div>
              ) : tab === "Preview" && bottomOpen ? (
                <PreviewInspector
                  workspace={workspace}
                  state={inspectorState}
                  dark={dark}
                />
              ) : tab === "Code" &&
                bottomOpen &&
                inspectorState.kind === "ready" ? (
                <CodeInspector
                  key={inspectionKey}
                  workspace={workspace}
                  state={inspectorState}
                />
              ) : tab === "History" &&
                bottomOpen &&
                inspectorState.kind === "ready" ? (
                <HistoryInspector
                  key={inspectionKey}
                  workspace={workspace}
                  state={inspectorState}
                  onVersion={(version) =>
                    setPinnedVersion(version ? { key: pinKey, version } : null)
                  }
                />
              ) : tab === "Build timeline" &&
                bottomOpen &&
                inspectorState.kind === "ready" ? (
                <TimelineInspector
                  key={inspectionKey}
                  workspace={workspace}
                  state={inspectorState}
                />
              ) : (
                <>
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
                </>
              )}
            </div>
          </section>
        </>
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
