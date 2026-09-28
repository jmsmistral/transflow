import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
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
import type { ApiLineageNodeV1 } from "../generated/contracts";
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
const nodeTypes = { dataset: Dataset };
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
}: {
  workspace: Workspace;
  dark?: boolean;
  cataloguePaths?: readonly string[];
  onVisible?: (identities: readonly string[]) => void;
  focusRequest?: { path: string; revision: number } | null;
}) {
  const [model] = useState(() => new GraphModel(workspace));
  const graph = useSyncExternalStore(model.subscribe, model.snapshot);
  const state = useSyncExternalStore(workspace.subscribe, workspace.snapshot);
  useEffect(() => model.connect(), [model]);
  const fingerprint =
    state.kind === "ready" ? state.value.context.fingerprint : "";
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
  dark,
  model,
  graph,
  workspace,
  active,
  focusRequest,
}: {
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
  const [depth, setDepth] = useState("1");
  const [direction, setDirection] =
    useState<Exploration["direction"]>("upstream");
  const [toolsOpen, setToolsOpen] = useState(false);
  const flow = useReactFlow<DatasetNode>();
  const flowStore = useStoreApi();
  const [legendOpen, setLegendOpen] = useState(false);
  const [colour, setColour] = useState<"resource" | "publication">("resource");
  const workspaceState = workspace.snapshot();
  const ready = workspaceState.kind === "ready" ? workspaceState.value : null;
  const chosen = [...selected].flatMap((id) =>
    graph.nodes.filter((node) => node.identity === id),
  );
  const expand = useCallback(
    (node: ApiLineageNodeV1, side: Exploration["direction"]) => {
      void model.toggle(node, side).then((ids) => {
        if (ids) setSelected(ids);
      });
    },
    [model],
  );
  const nodes: DatasetNode[] = useMemo(
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
  const edges: Edge[] = useMemo(() => {
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
  useEffect(() => {
    if (active) positions.sync(graph.nodes, graph.edges, graph.placement);
    return positions.stop;
  }, [positions, graph.nodes, graph.edges, graph.placement, active]);
  const changes = useCallback(
    (updates: NodeChange<DatasetNode>[]) => {
      for (const change of updates)
        if (change.type === "position" && change.position)
          positions.move(change.id, change.position);
      const selection = updates.filter((change) => change.type === "select");
      if (selection.length)
        setSelected((old) => {
          const next = new Set(old);
          for (const change of selection) {
            if (change.selected) next.add(change.id);
            else next.delete(change.id);
          }
          return next;
        });
    },
    [positions],
  );
  const focused = useRef<object | null>(null);
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
      focused.current = focusRequest;
      setSelected(new Set([node.identity]));
    });
    return () => cancelAnimationFrame(frame);
  }, [active, focusRequest, graph.busy, graph.nodes, layout, flow]);
  return (
    <>
      <div className="graph-floating-tools">
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
              onClick={() => setSelected(new Set())}
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
                if (ids) setSelected(ids);
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
          const bounds = flow.getNodesBounds(chosen.map((n) => n.identity));
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
        key={`${graph.busy}:${graph.message}:${layout.error}`}
        message={graph.message || layout.error}
        busy={graph.busy}
      />
      <div className="graph-diagram">
        <ReactFlow<DatasetNode>
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
              event.key.toLowerCase() === "a"
            ) {
              event.preventDefault();
              event.stopPropagation();
              setSelected(new Set(graph.nodes.map((n) => n.identity)));
              return;
            }
            if (!["Delete", "Backspace"].includes(event.key)) return;
            if (selected.size) {
              event.preventDefault();
              event.stopPropagation();
              model.remove(selected);
              setSelected(new Set());
            }
          }}

          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
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
                  nodes: chosen.map((n) => ({ id: n.identity })),
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
