import {
  useCallback,
  useEffect,
  useMemo,
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
  Handle,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Node,
  type NodeProps,
  type NodeChange,
  type Edge,
} from "@xyflow/react";
import type { ApiLineageNodeV1 } from "../generated/contracts";
import type { Workspace } from "../workspace";
import { Button } from "../components";
import {
  GraphModel,
  edgeId,
  label,
  type GraphState,
  type Exploration,
} from "./model";
import { CatalogueSearch } from "./Search";
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
    pinned: boolean;
    onPath: boolean;
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
      {side === "upstream" ? (expanded ? ">" : "<") : expanded ? "<" : ">"}
    </button>
  );
  return (
    <div
      className={`dataset-node ${data.value.external ? "foreign-node" : ""}`}
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
        <span aria-hidden="true">
          {data.value.external ? "↗" : data.value.producer ? "ƒ" : "▤"}
        </span>{" "}
        <span>{label(data.value)}</span>
        {data.pinned && <span title="Pinned">⌖</span>}
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
}: {
  workspace: Workspace;
  dark?: boolean;
  onVisible?: (identities: readonly string[]) => void;
}) {
  const [model] = useState(() => new GraphModel(workspace));
  const graph = useSyncExternalStore(model.subscribe, model.snapshot);
  const state = useSyncExternalStore(workspace.subscribe, workspace.snapshot);
  useEffect(() => model.connect(), [model]);
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
}: {
  dark: boolean;
  model: GraphModel;
  graph: GraphState;
  workspace: Workspace;
  active: boolean;
}) {
  const [positions] = useState(() => new Positions());
  const layout = useSyncExternalStore(positions.subscribe, positions.snapshot);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [reference, setReference] = useState("");
  const [depth, setDepth] = useState("1");
  const [direction, setDirection] =
    useState<Exploration["direction"]>("upstream");
  const [find, setFind] = useState("");
  const [large, setLarge] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  const flow = useReactFlow<DatasetNode>();
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
          workspace,
          busy: graph.busy,
          parentsExpanded: model.expanded(node, "upstream"),
          childrenExpanded: model.expanded(node, "downstream"),
          pinned: layout.pins.has(node.identity),
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
      graph.busy,
      model,
      workspace,
      graph.pathNodes,
      layout.positions,
      layout.pins,
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
          ...(edge.role === "validation" ? { strokeDasharray: "7 5" } : {}),
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
    if (active) positions.run(graph.nodes, graph.edges);
    return positions.stop;
  }, [positions, graph.nodes, graph.edges, active]);
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
  const inspect = (): void => {
    const identity = chosen[0]?.identity.match(/^dataset:([^:]+):([^:]+)$/);
    if (identity?.[1] && identity[2])
      void workspace.select({
        branch: workspaceState.selection.branch,
        ...(workspaceState.selection.fallback !== undefined
          ? { fallback: workspaceState.selection.fallback }
          : {}),
        origin: identity[1],
        dataset: identity[2],
        ...(workspaceState.selection.plan
          ? { plan: workspaceState.selection.plan }
          : {}),
      });
  };
  const matching = graph.nodes.filter((node) =>
    node.paths.some((path) => path.toLowerCase().includes(find.toLowerCase())),
  );
  return (
    <>
      <div className="graph-tools" hidden={!toolsOpen}>
        <CatalogueSearch
          workspace={workspace}
          active={active}
          add={(start) => {
            void model.explore(
              { start, direction: "upstream", depth: "0" },
              false,
              large,
            );
          }}
        />
        <form
          className="graph-query"
          onSubmit={(event) => {
            event.preventDefault();
            setToolsOpen(false);
            void model.explore(
              {
                start: reference.trim() || (chosen[0] ? label(chosen[0]) : ""),
                direction,
                depth,
              },
              false,
              large,
            );
          }}
        >
          <label>
            Dataset path
            <input
              aria-label="Dataset path to explore"
              placeholder="path/from/catalogue"
              value={reference}
              onChange={(event) => setReference(event.target.value)}
            />
          </label>
          <label>
            Direction
            <select
              value={direction}
              onChange={(event) =>
                setDirection(
                  event.target.value === "downstream"
                    ? "downstream"
                    : "upstream",
                )
              }
            >
              <option value="upstream">Ancestors</option>
              <option value="downstream">Descendants</option>
            </select>
          </label>
          <label>
            Depth
            <input
              aria-label="Traversal depth"
              placeholder="All"
              inputMode="numeric"
              value={depth}
              onChange={(event) => setDepth(event.target.value)}
            />
          </label>
          <Button
            type="submit"
            disabled={
              !ready?.context.graph ||
              graph.busy ||
              (!reference.trim() && chosen.length !== 1)
            }
          >
            Explore
          </Button>
        </form>
        <details className="graph-help">
          <summary>Navigation help</summary>{" "}
          <p className="graph-hint">
            Blank depth means all reachable local datasets; each action loads at
            most 100 nodes and 100 edges. Drag or use arrow keys to pin
            positions. Shift/Cmd/Ctrl adds to selection. Paths follow the first
            selected dataset to the second. Removing nodes changes only this
            view.
          </p>
        </details>
        <div className="graph-find">
          <label>
            Find visible
            <input
              value={find}
              onChange={(event) => setFind(event.target.value)}
            />
          </label>
          <Button
            disabled={!matching.length}
            onClick={() => {
              setSelected(new Set(matching.map((node) => node.identity)));
              void flow.fitView({
                nodes: matching.map((node) => ({ id: node.identity })),
                duration: 200,
              });
            }}
          >
            Select matches ({matching.length})
          </Button>
        </div>
      </div>
      <div className="graph-actions">
        <Button
          aria-expanded={toolsOpen}
          onClick={() => setToolsOpen(!toolsOpen)}
        >
          Explore / search
        </Button>
        <Button
          onClick={() => {
            void flow.fitView({ duration: 200 });
          }}
        >
          Fit view
        </Button>
        <Button
          disabled={!chosen.length}
          onClick={() => {
            void flow.fitView({
              nodes: chosen.map((node) => ({ id: node.identity })),
              duration: 200,
            });
          }}
        >
          Fit selection
        </Button>
        <details className="graph-options">
          <summary>Selection and layout</summary>
          <div className="graph-actions">
            {" "}
            <label>
              <input
                type="checkbox"
                checked={large}
                onChange={(event) => setLarge(event.target.checked)}
              />{" "}
              Allow more than 500 visible datasets
            </label>
            <Button
              disabled={!chosen.length}
              onClick={() => {
                model.remove(selected);
                setSelected(new Set());
              }}
            >
              Remove from view
            </Button>
            <Button
              disabled={!chosen.length}
              onClick={() => positions.unpin(selected)}
            >
              Unpin selection
            </Button>
            <Button
              disabled={
                chosen.length !== 1 ||
                !chosen[0]?.identity.startsWith("dataset:")
              }
              onClick={inspect}
            >
              Inspect selection
            </Button>
            <Button
              disabled={chosen.length !== 2 || graph.busy}
              onClick={() => {
                const [start, end] = chosen;
                if (start && end)
                  void model.explore(
                    {
                      start: label(start),
                      end: label(end),
                      direction: "downstream",
                      depth: "",
                    },
                    false,
                    large,
                  );
              }}
            >
              Show paths
            </Button>
            {chosen.length === 2 && (
              <span>
                Path direction: {chosen[0] && label(chosen[0])} →{" "}
                {chosen[1] && label(chosen[1])}
              </span>
            )}
            <Button
              disabled={!graph.nodes.length}
              onClick={() => positions.run(graph.nodes, graph.edges, true)}
            >
              Relayout all (clear pins)
            </Button>
            {layout.error && (
              <Button onClick={() => positions.run(graph.nodes, graph.edges)}>
                Retry layout
              </Button>
            )}
          </div>
        </details>
      </div>
      <details className="graph-summary">
        <summary>
          {graph.nodes.length} visible datasets · {edges.length} visible edges
          {graph.busy ? " · Loading batch…" : ""}
          {layout.busy ? " · Arranging graph…" : ""}
        </summary>
        <p className="graph-counts" role="status">
          {graph.nodes.length} visible datasets · {edges.length} visible edges ·{" "}
          {graph.edges.length - edges.length} edges awaiting endpoints.{" "}
          {graph.busy ? "Loading batch… " : ""}
          {layout.busy ? "Arranging graph… " : ""}
          {graph.page &&
            `Last query: ${graph.page.total_nodes} nodes / ${graph.page.total_edges} edges; ${graph.page.remaining_nodes} nodes / ${graph.page.remaining_edges} edges pending; ${graph.page.omitted_nodes} nodes / ${graph.page.omitted_edges} edges beyond depth.`}
        </p>
      </details>
      {graph.page?.next_cursor && graph.query && (
        <Button
          disabled={graph.busy}
          onClick={() => {
            if (graph.query) void model.explore(graph.query, true, large);
          }}
        >
          Load next graph batch
        </Button>
      )}
      {graph.message && <p role="alert">{graph.message}</p>}
      {layout.error && <p role="alert">{layout.error}</p>}
      {!graph.nodes.length && (
        <p className="graph-empty">
          {ready?.context.graph
            ? "Select a catalogue dataset, or open Explore / search to enter a path."
            : "No validated graph retained in this context. Validate or build through the CLI, then refresh."}
        </p>
      )}
      <div className="graph-diagram">
        <ReactFlow<DatasetNode>
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          onNodesChange={changes}
          nodesConnectable={false}
          edgesReconnectable={false}
          deleteKeyCode={null}
          multiSelectionKeyCode={["Shift", "Meta", "Control"]}
          selectionOnDrag
          panOnDrag={[1, 2]}
          minZoom={0.05}
          maxZoom={2}
          fitView
          aria-label="Dataset lineage diagram"
          colorMode={dark ? "dark" : "light"}
        >
          <Background />
          <Controls showInteractive={false} />
        </ReactFlow>
      </div>
      <details className="graph-legend">
        <summary>Legend</summary>
        <p>
          Solid: data · Dashed: validation-only · ↗: foreign read boundary.
          Provider code is never executed by graph navigation. Freshness and
          quality overlays arrive separately.
        </p>
      </details>
    </>
  );
}
