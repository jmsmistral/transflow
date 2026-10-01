import { useEffect, useState } from "react";
import type {
  ApiDatasetV1,
  ApiLineageNodeV1,
  ExecutionJsonV1,
} from "../generated/contracts";
import type { Workspace } from "../workspace";
import { label } from "./model";
import { paint } from "./overlays";
import { formatTimestamp } from "../date";
function field(
  value: ExecutionJsonV1 | undefined,
  key: string,
): ExecutionJsonV1 | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (Reflect.get(value, key) as ExecutionJsonV1)
    : undefined;
}
function scalar(value: ExecutionJsonV1 | undefined): string {
  return typeof value === "string" ? value : "Unknown";
}
export function DatasetTooltip({
  node,
  workspace,
}: {
  node: ApiLineageNodeV1;
  workspace: Workspace;
}) {
  const [detail, setDetail] = useState<{
    value: ApiDatasetV1;
    fingerprint: string;
  } | null>(null);
  const [message, setMessage] = useState("Loading metadata…");
  const state = workspace.snapshot();
  const fingerprint =
    state.kind === "ready" ? state.value.context.fingerprint : "";
  useEffect(() => {
    const request = new AbortController();
    const identity = node.identity.match(/^dataset:([^:]+):([^:]+)$/);
    if (!identity) return;
    void workspace
      .read(
        "ApiDatasetV1",
        `/api/v1/datasets/${encodeURIComponent(identity[2] ?? "")}`,
        { origin_workspace: identity[1] ?? "" },
        request.signal,
      )
      .then(
        (result) => {
          if (!request.signal.aborted) {
            setDetail({ value: result, fingerprint });
            setMessage("");
          }
        },
        () => {
          if (!request.signal.aborted)
            setMessage("Metadata unavailable in this context");
        },
      );
    return () => request.abort();
  }, [node.identity, workspace, fingerprint]);
  if (!detail || detail.fingerprint !== fingerprint) return null;
  const head = detail.value.head;
  const published = field(head, "published_at_us");
  const fields = field(field(head, "schema"), "fields");
  return (
    <div className="dataset-tooltip" role="tooltip">
      <header>
        <strong>{label(node)}</strong>
        <span>({state.selection.branch})</span>
      </header>
      <p>
        {node.external
          ? "↗ External dataset · provider owned"
          : node.producer
            ? "ƒ Transform dataset"
            : "▤ Dataset"}
      </p>
      <dl>
        <dt>Path</dt>
        <dd>{label(node)}</dd>
        <dt>Resolved branch</dt>
        <dd>{scalar(field(head, "resolved_branch"))}</dd>
        <dt>Last published</dt>
        <dd>{formatTimestamp(published)}</dd>
        <dt>Rows / size</dt>
        <dd>
          {scalar(field(head, "row_count"))} rows ·{" "}
          {scalar(field(head, "byte_count"))} bytes
        </dd>
        <dt>Columns</dt>
        <dd>{Array.isArray(fields) ? fields.length : "Unknown"}</dd>
        <dt>Freshness / quality</dt>
        <dd>
          {paint(node, "freshness").label} · {paint(node, "health").label}
        </dd>
        <dt>Latest attempt</dt>
        <dd>
          {paint(node, "status").label}
          {node.overlay?.version ? " · Published data available" : ""}
        </dd>
      </dl>
      {message && (
        <p className="muted">
          {node.identity.startsWith("pending:")
            ? "Unregistered declaration"
            : message}
        </p>
      )}
    </div>
  );
}
