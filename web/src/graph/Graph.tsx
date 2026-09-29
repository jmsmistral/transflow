import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type SetStateAction,
} from "react";
import {
  Background,
  NodeToolbar,
  BaseEdge,
  getBezierPath,
  MarkerType,
  type EdgeProps,
  Controls,
  ControlButton,
  Handle,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  useStoreApi,
  type Node,
  type NodeProps,
  type NodeChange,
  type Edge,
} from "@xyflow/react";
import type { GraphViewV1, ApiLineageNodeV1 } from "../generated/contracts";
import type { Workspace } from "../workspace";
import { Icon } from "../Icons";
import { Button } from "../components";
import {
  GraphModel,
  edgeId,
  label,
  type GraphState,
  type Exploration,
} from "./model";
import { DatasetTooltip } from "./Tooltip";
import { ViewEditor } from "./ViewEditor";
import { ViewHistory, rollup, type Visual } from "./view";
import { NoteText } from "./Note";
import { same } from "../api/validate";
import { queryFor } from "../workspace";
import { Positions } from "./positions";
import "@xyflow/react/dist/style.css";

type DatasetNode = Node<
  {
    value: ApiLineageNodeV1;
    kind: string;
    workspace: Workspace;
    parentsExpanded: boolean;
    childrenExpanded: boolean;
    busy: boolean;
    onPath: boolean;
    colour: "resource" | "publication";
    expand: (
      node: ApiLineageNodeV1,
      direction: Exploration["direction"],
    ) => void;
  },
  "dataset"
>;
function Dataset({ data }: NodeProps<DatasetNode>) {
  const [hovered, setHovered] = useState(false);
  const parents = data.value.parent_count !== "0" && !data.value.external;
  const children = data.value.child_count !== "0";
  const arrow = (
    side: Exploration["direction"],
    expanded: boolean,
    count: string,
  ) => (
    <button
      type="button"
      className={`node-expansion ${side === "upstream" ? "node-parents" : "node-children"} nodrag nopan`}
      disabled={data.busy}
      aria-label={`${expanded ? "Retract" : "Expand"} ${side === "upstream" ? "parents" : "children"} of ${label(data.value)}`}
      title={`${count} ${side === "upstream" ? "parents" : "children"} · ${expanded ? "Retract" : "Expand all"}`}
      aria-expanded={expanded}
      onKeyDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        setHovered(false);
        data.expand(data.value, side);
      }}
    >
      <Icon
        name={
          side === "upstream"
            ? expanded
              ? "right"
              : "left"
            : expanded
              ? "left"
              : "right"
        }
      />
    </button>
  );
  return (
    <div
      className={`dataset-node colour-${data.colour === "resource" ? data.value.resource_type : data.value.publication} ${data.value.publication === "missing" ? "unbuilt-node" : ""}`}
    >
      <Handle type="target" position={Position.Left} isConnectable={false} />
      {parents &&
        arrow("upstream", data.parentsExpanded, data.value.parent_count)}
      <button
        className="node-label"
        aria-label={`Details for ${label(data.value)}`}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        onFocus={() => setHovered(true)}
        onBlur={() => setHovered(false)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setHovered(false);
            e.stopPropagation();
          }
        }}
        onClick={() => setHovered(true)}
      >
        <span
          className={`resource-icon ${data.value.external ? "external-icon" : ""}`}
          aria-hidden="true"
        >
          {data.value.producer ? (
            "ƒ"
          ) : (
            <Icon name={data.value.external ? "external" : "table"} />
          )}
        </span>{" "}
        <span>{label(data.value)}</span>
      </button>
      {children &&
        arrow("downstream", data.childrenExpanded, data.value.child_count)}
      <Handle type="source" position={Position.Right} isConnectable={false} />
      <NodeToolbar isVisible={hovered} position={Position.Top} offset={12}>
        <DatasetTooltip node={data.value} workspace={data.workspace} />
      </NodeToolbar>
    </div>
  );
}
type NoteNode = Node<{ text: string; markdown: boolean }, "note">;
type GroupNode = Node<
  { name: string; summary: string; collapsed: boolean; toggle: () => void },
  "viewGroup"
>;
type CanvasNode = DatasetNode | NoteNode | GroupNode;
function Annotation({ data }: NodeProps<NoteNode>) {
  return (
    <div className="view-note">
      <NoteText text={data.text} markdown={data.markdown} />
    </div>
  );
}
function PresentationGroup({ data }: NodeProps<GroupNode>) {
  return (
    <div className={`view-group ${data.collapsed ? "collapsed" : ""}`}>
      <Handle type="target" position={Position.Left} isConnectable={false} />
      <button className="nodrag nopan" onClick={data.toggle}>
        {data.collapsed ? "Expand" : "Collapse"} {data.name}
      </button>
      <small>{data.summary}</small>
      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}
const nodeTypes = {
  dataset: Dataset,
  note: Annotation,
  viewGroup: PresentationGroup,
};
function Dependency(props: EdgeProps) {
  const [path] = getBezierPath(props);
  return (
    <g>
      <title>{props.label}</title>
      <BaseEdge
        id={props.id}
        path={path}
        {...(props.style ? { style: props.style } : {})}
        {...(props.markerEnd ? { markerEnd: props.markerEnd } : {})}
      />
    </g>
  );
}
const edgeTypes = { dependency: Dependency };

export function GraphExplorer({
  workspace,
  dark = false,
  onVisible,
  focusRequest,
  cataloguePaths,
  catalogueFilter = "",
  onFilter,
}: {
  catalogueFilter?: string;
  onFilter?: (filter: string) => void;
  workspace: Workspace;
  dark?: boolean;
  cataloguePaths?: readonly string[];
  onVisible?: (identities: readonly string[]) => void;
  focusRequest?: { path: string; revision: number } | null;
}) {
  const [model] = useState(() => new GraphModel(workspace));
  const [opened, setOpened] = useState<{
    view: GraphViewV1;
    context: string;
  } | null>(null);
  const openView = useCallback(
    async (view: GraphViewV1) => {
      const selector = {
        branch: view.selector.branch,
        fallback: view.selector.fallback,
        ...(view.selector.mode === "snapshot" && view.selector.graph
          ? { sourceGraph: view.selector.graph }
          : {}),
      };
      if (!same(queryFor(workspace.snapshot().selection), queryFor(selector)))
        await workspace.select(selector);
      await workspace.settled();
      if (workspace.snapshot().kind !== "ready")
        throw new Error("The saved source context is unavailable.");
      while (true) {
        const context = workspace.snapshot();
        if (
          context.kind !== "ready" ||
          !same(queryFor(context.selection), queryFor(selector))
        )
          throw new Error("The context changed while opening the view.");
        try {
          await model.reopen(view.datasets.map((n) => n.identity));
          break;
        } catch (error) {
          // A concurrent committed-event refresh may cancel the read. Retry only
          // in the same requested selector, never across a user context switch.
          if (
            workspace.snapshot() === context &&
            !(error instanceof DOMException && error.name === "AbortError")
          )
            throw new Error(
              `Saved membership could not load: ${error instanceof Error ? error.message : "read failed"}`,
            );
          await workspace.settled();
        }
      }
      const ready = workspace.snapshot();
      if (ready.kind !== "ready") throw new Error("The context changed.");
      setOpened({ view, context: ready.value.context.fingerprint });
      onFilter?.(view.filters.path);
    },
    [model, workspace, onFilter],
  );
  const shared = useRef(false);

  const graph = useSyncExternalStore(model.subscribe, model.snapshot);
  const state = useSyncExternalStore(workspace.subscribe, workspace.snapshot);
  useEffect(() => model.connect(), [model]);
  const fingerprint =
    state.kind === "ready" ? state.value.context.fingerprint : "";
  useEffect(() => {
    const id = new URLSearchParams(window.location.search).get("view");
    if (!fingerprint || !id || shared.current) return;
    shared.current = true;
    const request = new AbortController();
    void workspace.client
      .read(
        "GraphViewV1",
        `/api/v1/views/${encodeURIComponent(id)}`,
        {},
        request.signal,
      )
      .catch((error: unknown) => {
        throw new Error(
          `Saved document could not load: ${error instanceof Error ? error.message : "read failed"}`,
        );
      })
      .then((r) => openView(r.data))
      .catch((error: unknown) => {
        window.alert(
          error instanceof Error
            ? error.message
            : "The shared view is unavailable on this coordinator.",
        );
      });
  }, [fingerprint, workspace, openView]);
  useEffect(() => {
    if (fingerprint && cataloguePaths?.length) model.prefetch(cataloguePaths);
  }, [model, fingerprint, cataloguePaths]);
  useEffect(() => {
    if (fingerprint && focusRequest) void model.add(focusRequest.path);
  }, [model, fingerprint, focusRequest]);

  useEffect(
    () => onVisible?.(graph.nodes.map((n) => n.identity)),
    [graph.nodes, onVisible],
  );
  const ready =
    state.kind === "ready" && state.value.context.fingerprint === graph.context;
  return graph.context ? (
    <div className="graph-explorer" hidden={!ready}>
      <ReactFlowProvider key={graph.context}>
        <GraphView
          catalogueFilter={catalogueFilter}
          onFilter={onFilter}
          opened={opened?.context === fingerprint ? opened.view : null}
          onOpen={openView}
          dark={dark}
          model={model}
          graph={graph}
          workspace={workspace}
          active={ready}
          focusRequest={focusRequest ?? null}
        />
      </ReactFlowProvider>
    </div>
  ) : null;
}
function GraphView({
  catalogueFilter,
  onFilter,
  opened,
  onOpen,
  dark,
  model,
  graph,
  workspace,
  active,
  focusRequest,
}: {
  catalogueFilter: string;
  onFilter: ((filter: string) => void) | undefined;
  opened: GraphViewV1 | null;
  onOpen: (view: GraphViewV1) => Promise<void>;
  dark: boolean;
  model: GraphModel;
  graph: GraphState;
  workspace: Workspace;
  active: boolean;
  focusRequest: { path: string; revision: number } | null;
}) {
  const [positions] = useState(() => new Positions());
  const layout = useSyncExternalStore(positions.subscribe, positions.snapshot);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const focused = useRef<object | null>(null);
  // A direct selection supersedes pending catalogue focus, even before its frame runs.
  const selectNodes = useCallback(
    (next: SetStateAction<ReadonlySet<string>>) => {
      focused.current = focusRequest;
      setSelected(next);
    },
    [focusRequest],
  );
  const [depth, setDepth] = useState("1");
  const [direction, setDirection] =
    useState<Exploration["direction"]>("upstream");
  const [toolsOpen, setToolsOpen] = useState(false);
  const flow = useReactFlow<CanvasNode>();
  const flowStore = useStoreApi();
  const [legendOpen, setLegendOpen] = useState(false);
  const [colour, setColour] = useState<"resource" | "publication">("resource");
  const [groups, setGroups] = useState<GraphViewV1["groups"]>([]);
  const [annotations, setAnnotations] = useState<GraphViewV1["annotations"]>(
    [],
  );
  const [viewport, setViewport] = useState({ x: 0, y: 0, zoom: 1 });
  const [notice, setNotice] = useState("");
  const [history] = useState(() => new ViewHistory());
  const [, refreshHistory] = useState(0);
  const dragging = useRef(false);
  const restoring = useRef(false);
  const openedRef = useRef<GraphViewV1 | null>(null);
  const validGroups = useMemo(
    () =>
      groups
        .map((g) => ({
          ...g,
          members: g.members.filter((id) =>
            graph.nodes.some((n) => n.identity === id),
          ),
        }))
        .filter((g) => g.members.length),
    [groups, graph.nodes],
  );
  const visual: Visual = {
    nodes: graph.nodes,
    edges: graph.edges,
    positions: layout.positions,
    selected: [...selected].filter((id) =>
      graph.nodes.some((n) => n.identity === id),
    ),
    groups: validGroups,
    annotations,
    viewport,
    colour,
  };
  const visualRef = useRef(visual);
  useEffect(() => {
    visualRef.current = visual;
  });
  const apply = useCallback(
    (v: Visual) => {
      restoring.current = true;
      model.restore(v);
      positions.restore(v.positions);
      selectNodes(new Set(v.selected));
      setGroups(v.groups);
      setAnnotations(v.annotations);
      setColour(v.colour);
      setViewport(v.viewport);
      void flow.setViewport(v.viewport);
    },
    [model, positions, selectNodes, flow],
  );
  useEffect(() => {
    if (!opened || openedRef.current === opened) return;
    openedRef.current = opened;
    const v: Visual = {
      ...visualRef.current,
      positions: Object.fromEntries(
        opened.datasets.map((n) => [n.identity, n.position]),
      ),
      selected: [],
      groups: opened.groups,
      annotations: opened.annotations,
      viewport: opened.viewport,
      colour: opened.colour,
    };
    apply(v);
    history.reset(v);
    refreshHistory((n) => n + 1);
  }, [opened, apply, history]);
  useEffect(() => {
    if (
      graph.busy ||
      dragging.current ||
      graph.nodes.some((n) => !layout.positions[n.identity])
    )
      return;
    const timer = setTimeout(() => {
      if (restoring.current) {
        restoring.current = false;
        return;
      }
      history.observe(visualRef.current);
      refreshHistory((n) => n + 1);
    }, 0);
    return () => clearTimeout(timer);
  }, [
    graph.nodes,
    graph.edges,
    graph.busy,
    layout.positions,
    selected,
    validGroups,
    annotations,
    viewport,
    colour,
    history,
  ]);
  const undo = (redo = false) => {
    if (!restoring.current) history.observe(visualRef.current);
    const next = redo ? history.redo() : history.undo();
    if (next) apply(next);
    refreshHistory((n) => n + 1);
  };
  const collapsed = new Map(
    validGroups
      .filter((g) => g.collapsed)
      .flatMap((g) => g.members.map((id) => [id, g.id] as const)),
  );
  const workspaceState = workspace.snapshot();
  const ready = workspaceState.kind === "ready" ? workspaceState.value : null;
  const chosen = [...selected].flatMap((id) =>
    graph.nodes.filter((node) => node.identity === id),
  );
  const expand = useCallback(
    (node: ApiLineageNodeV1, side: Exploration["direction"]) => {
      void model.toggle(node, side).then((ids) => {
        if (ids) selectNodes(ids);
      });
    },
    [model, selectNodes],
  );
  const datasetNodes: DatasetNode[] = useMemo(
    () =>
      graph.nodes.map((node, index) => ({
        id: node.identity,
        type: "dataset",
        position: layout.positions[node.identity] ?? {
          x: (index % 4) * 300,
          y: Math.floor(index / 4) * 180,
        },
        selected: selected.has(node.identity),
        ariaLabel: `${label(node)}, ${node.external ? "read-only foreign boundary" : "dataset"}`,
        data: {
          value: node,
          colour,
          workspace,
          busy: graph.busy,
          parentsExpanded: model.expanded(node, "upstream"),
          childrenExpanded: model.expanded(node, "downstream"),
          onPath: graph.pathNodes?.has(node.identity) ?? false,
          expand,
          kind:
            ready?.datasets.entries.find((entry) =>
              node.paths.includes(entry.path),
            )?.kind ??
            (node.external
              ? "external"
              : node.producer
                ? "producer"
                : "dataset"),
        },
      })),
    [
      graph.nodes,
      colour,
      graph.busy,
      model,
      workspace,
      graph.pathNodes,
      layout.positions,
      selected,
      expand,
      ready,
    ],
  );
  const nodes: CanvasNode[] = [
    ...validGroups.map((g): GroupNode => {
      const members = g.members.map(
        (id) => layout.positions[id] ?? { x: 0, y: 0 },
      );
      const x = g.collapsed
        ? g.position.x
        : Math.min(...members.map((p) => p.x)) - 16;
      const y = g.collapsed
        ? g.position.y
        : Math.min(...members.map((p) => p.y)) - 52;
      return {
        id: g.id,
        type: "viewGroup",
        position: { x, y },
        style: {
          width: g.collapsed
            ? 280
            : Math.max(...members.map((p) => p.x)) + 276 - x,
          height: g.collapsed
            ? 80
            : Math.max(...members.map((p) => p.y)) + 64 - y,
        },
        zIndex: -1,
        selectable: g.collapsed,
        selected: g.collapsed && g.members.every((id) => selected.has(id)),
        data: {
          name: g.name,
          summary: rollup(g, graph.nodes),
          collapsed: g.collapsed,
          toggle: () =>
            setGroups((old) =>
              old.map((v) =>
                v.id === g.id ? { ...v, collapsed: !v.collapsed } : v,
              ),
            ),
        },
      };
    }),
    ...datasetNodes.filter((n) => !collapsed.has(n.id)),
    ...annotations.map((n): NoteNode => ({
      id: n.id,
      type: "note",
      position: n.position,
      selectable: false,
      data: { text: n.text, markdown: n.format === "markdown" },
    })),
  ];
  const rawEdges: Edge[] = useMemo(() => {
    const ids = new Set(graph.nodes.map((node) => node.identity));
    return graph.edges
      .filter((edge) => ids.has(edge.parent) && ids.has(edge.consumer))
      .map((edge) => ({
        id: edgeId(edge),
        type: "dependency",
        markerEnd: { type: MarkerType.ArrowClosed },
        source: edge.parent,
        target: edge.consumer,
        label: `${edge.alias} · ${edge.role}`,
        ariaLabel: `${edge.alias}: ${edge.role} dependency, branch ${edge.declared_branch.name ?? edge.declared_branch.kind}, ${edge.stop_branch_fallback ? "fallback blocked" : "fallback permitted"}, ${edge.checks.length} checks`,
        style: {
          ...(edge.role === "validation" ? { stroke: "var(--accent)" } : {}),
          strokeWidth:
            graph.pathNodes?.has(edge.parent) &&
            graph.pathNodes.has(edge.consumer)
              ? 3
              : 1,
        },
        deletable: false,
        focusable: true,
      }));
  }, [graph.nodes, graph.edges, graph.pathNodes]);
  const edges = rawEdges.flatMap((edge) => {
    const source = collapsed.get(edge.source) ?? edge.source,
      target = collapsed.get(edge.target) ?? edge.target;
    return source === target ? [] : [{ ...edge, source, target }];
  });
  useEffect(() => {
    if (active) positions.sync(graph.nodes, graph.edges, graph.placement);
    return positions.stop;
  }, [positions, graph.nodes, graph.edges, graph.placement, active]);
  const changes = (updates: NodeChange<CanvasNode>[]) => {
    for (const change of updates)
      if (change.type === "position" && change.position) {
        const position = change.position;
        const note = annotations.find((n) => n.id === change.id),
          group = validGroups.find((g) => g.id === change.id);
        if (note)
          setAnnotations((old) =>
            old.map((n) => (n.id === change.id ? { ...n, position } : n)),
          );
        else if (group) {
          const old =
            nodes.find((n) => n.id === group.id)?.position ?? group.position;
          for (const id of group.members) {
            const p = layout.positions[id];
            if (p)
              positions.move(id, {
                x: p.x + position.x - old.x,
                y: p.y + position.y - old.y,
              });
          }
          setGroups((old) =>
            old.map((g) => (g.id === group.id ? { ...g, position } : g)),
          );
        } else positions.move(change.id, position);
      }
    const selection = updates.filter((change) => change.type === "select");
    if (selection.length)
      selectNodes((old) => {
        const next = new Set(old);
        for (const change of selection) {
          const members = validGroups.find((g) => g.id === change.id)
            ?.members ?? [change.id];
          for (const id of members) {
            if (change.selected) next.add(id);
            else next.delete(id);
          }
        }
        return next;
      });
  };
  useEffect(() => {
    if (
      !active ||
      !focusRequest ||
      focused.current === focusRequest ||
      graph.busy ||
      layout.busy
    )
      return;
    const node = graph.nodes.find((n) => n.paths.includes(focusRequest.path));
    const position = node && layout.positions[node.identity];
    if (!node || !position) return;
    const frame = requestAnimationFrame(() => {
      if (focused.current === focusRequest) return;
      focused.current = focusRequest;
      setSelected(new Set([node.identity]));
    });
    return () => cancelAnimationFrame(frame);
  }, [active, focusRequest, graph.busy, graph.nodes, layout, flow]);
  return (
    <>
      <div className="graph-floating-tools">
        <ViewEditor
          filter={catalogueFilter}
          onFilter={onFilter}
          workspace={workspace}
          visual={visual}
          opened={opened}
          onOpen={onOpen}
          onGroups={setGroups}
          onNotes={setAnnotations}
          onMessage={setNotice}
        />
        <Button
          aria-label="Undo view change"
          disabled={!history.canUndo}
          onClick={() => undo()}
        >
          <Icon name="undo" />
        </Button>
        <Button
          aria-label="Redo view change"
          disabled={!history.canRedo}
          onClick={() => undo(true)}
        >
          <Icon name="redo" />
        </Button>
        <Button disabled title="Layout options are coming in a later iteration">
          <Icon name="layout" /> Layout
        </Button>
        <details className="graph-options">
          <summary>
            <Icon name="select" /> Select
          </summary>
          <div className="graph-actions">
            <Button
              disabled={!chosen.length}
              onClick={() => selectNodes(new Set())}
            >
              Clear all
            </Button>
          </div>
        </details>
        <Button
          disabled={!chosen.length || graph.busy}
          aria-expanded={toolsOpen}
          onClick={() => setToolsOpen(!toolsOpen)}
        >
          <Icon name="expand" /> Expand
        </Button>
      </div>
      {toolsOpen && chosen.length > 0 && (
        <div className="graph-tools">
          <form
            className="graph-query"
            onSubmit={(event) => {
              event.preventDefault();
              setToolsOpen(false);
              void model.expand(chosen, direction, depth).then((ids) => {
                if (ids) selectNodes(ids);
              });
            }}
          >
            <label>
              Direction
              <select
                value={direction}
                onChange={(e) =>
                  setDirection(
                    e.target.value === "downstream" ? "downstream" : "upstream",
                  )
                }
              >
                <option value="downstream">Forward (children)</option>
                <option value="upstream">Backward (parents)</option>
              </select>
            </label>
            <label>
              Levels
              <input
                aria-label="Expansion levels"
                inputMode="numeric"
                placeholder="All"
                value={depth}
                onChange={(e) => setDepth(e.target.value)}
              />
            </label>
            <Button type="submit" disabled={graph.busy}>
              Expand selection
            </Button>
          </form>
          <p className="graph-hint">
            Blank levels expands all reachable local datasets. Drag to pan;
            Shift-drag selects a box. Delete/Backspace removes the selection
            from this view. Node positions and zoom stay unchanged.
          </p>
        </div>
      )}
      <button
        className="graph-summary"
        disabled={!chosen.length}
        title="Centre selection"
        onClick={() => {
          const bounds = flow.getNodesBounds([
            ...new Set(
              chosen.map((n) => collapsed.get(n.identity) ?? n.identity),
            ),
          ]);
          void flow.setCenter(
            bounds.x + bounds.width / 2,
            bounds.y + bounds.height / 2,
            { zoom: flow.getZoom(), duration: 200 },
          );
        }}
      >
        {chosen.length} {chosen.length === 1 ? "node" : "nodes"} selected
      </button>
      <GraphNotice
        key={`${graph.busy}:${graph.message}:${layout.error}:${notice}`}
        message={graph.message || layout.error || notice}
        busy={graph.busy}
      />
      <div className="graph-diagram">
        <ReactFlow<CanvasNode>
          tabIndex={0}
          onMouseDown={(event) => {
            if (
              (event.target as HTMLElement).classList.contains(
                "react-flow__pane",
              )
            )
              event.currentTarget.focus();
          }}
          onKeyDownCapture={(event) => {
            const target = event.target as HTMLElement;
            if (
              target.closest(
                "input, textarea, select, [contenteditable=true]",
              ) ||
              (target.closest("button") && !target.closest(".react-flow__node"))
            )
              return;
            if (
              (event.metaKey || event.ctrlKey) &&
              event.key.toLowerCase() === "z"
            ) {
              event.preventDefault();
              event.stopPropagation();
              undo(event.shiftKey);
              return;
            }
            if (
              (event.metaKey || event.ctrlKey) &&
              event.key.toLowerCase() === "a"
            ) {
              event.preventDefault();
              event.stopPropagation();
              selectNodes(new Set(graph.nodes.map((n) => n.identity)));
              return;
            }
            if (!["Delete", "Backspace"].includes(event.key)) return;
            if (selected.size) {
              event.preventDefault();
              event.stopPropagation();
              model.remove(selected);
              selectNodes(new Set());
            }
          }}

          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          onNodeDragStart={() => {
            history.observe(visualRef.current);
            dragging.current = true;
          }}
          onNodeDragStop={() => {
            dragging.current = false;
            history.observe(visualRef.current);
            refreshHistory((n) => n + 1);
          }}
          onMoveEnd={(_event, next) => setViewport(next)}
          onNodesChange={changes}
          onSelectionEnd={() =>
            queueMicrotask(() =>
              flowStore.setState({ nodesSelectionActive: false }),
            )
          }
          proOptions={{ hideAttribution: true }}
          nodesConnectable={false}
          edgesReconnectable={false}
          deleteKeyCode={null}
          multiSelectionKeyCode={["Shift", "Meta", "Control"]}
          selectionOnDrag={false}
          selectionKeyCode="Shift"
          panOnDrag={true}
          minZoom={0.05}
          maxZoom={2}
          aria-label="Dataset lineage diagram"
          colorMode={dark ? "dark" : "light"}
        >
          <Background />
          <Controls showInteractive={false} showFitView={false}>
            <ControlButton
              title="Fit view"
              aria-label="Fit view"
              onClick={() => {
                void flow.fitView({ duration: 200 });
              }}
            >
              <Icon name="fit" />
            </ControlButton>
            <ControlButton
              title="Fit selection"
              aria-label="Fit selection"
              disabled={!chosen.length}
              onClick={() => {
                void flow.fitView({
                  nodes: [
                    ...new Set(
                      chosen.map(
                        (n) => collapsed.get(n.identity) ?? n.identity,
                      ),
                    ),
                  ].map((id) => ({ id })),
                  duration: 200,
                });
              }}
            >
              <span className="fit-selection-icon" />
            </ControlButton>
          </Controls>
        </ReactFlow>
      </div>
      <div className="graph-colour-controls">
        <Button
          aria-label="Toggle legend"
          title="Legend"
          aria-expanded={legendOpen}
          onClick={() => setLegendOpen(!legendOpen)}
        >
          <Icon name="legend" />
        </Button>
        <select
          aria-label="Node colouring"
          value={colour}
          onChange={(e) =>
            setColour(
              e.target.value === "publication" ? "publication" : "resource",
            )
          }
        >
          <option value="resource">Resource Type</option>
          <option value="publication">Publication</option>
        </select>
        {legendOpen && (
          <div className="graph-colour-legend" aria-label="Graph legend">
            {(colour === "resource" ? resourceLabels : publicationLabels).map(
              ([key, name]) => (
                <div key={key}>
                  <span className={`legend-swatch colour-${key}`} />
                  {name}
                  <span className="muted">
                    {
                      graph.nodes.filter(
                        (n) =>
                          (colour === "resource"
                            ? n.resource_type
                            : n.publication) === key,
                      ).length
                    }
                  </span>
                </div>
              ),
            )}
            <p>
              Counts show visible datasets. Dashed border: no publication on
              this branch or its fallbacks. Unknown is not missing. Publication
              does not verify freshness or dataset files.
            </p>
          </div>
        )}
      </div>
    </>
  );
}

function GraphNotice({ message, busy }: { message: string; busy: boolean }) {
  const [dismissed, setDismissed] = useState<string | null>(null);
  useEffect(() => {
    if (!message) return;
    const timeout = window.setTimeout(() => setDismissed(message), 4000);
    return () => window.clearTimeout(timeout);
  }, [message]);
  return (
    <div className="graph-notice" role="status" aria-live="polite">
      {busy ? "Adding datasets…" : message !== dismissed ? message : ""}
    </div>
  );
}

const resourceLabels = [
  ["polars_transform", "Polars Transform"],
  ["sql_transform", "SQL Transform"],
  ["external", "External Dataset"],
  ["dataset", "Dataset"],
  ["unknown", "Unknown"],
] as const;
const publicationLabels = [
  ["published", "Published"],
  ["missing", "Not built on these branches"],
  ["unknown", "Unknown"],
] as const;
