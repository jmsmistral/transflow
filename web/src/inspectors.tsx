import { useEffect, useMemo, useState } from "react";
import type {
  ApiDatasetV1,
  ApiDatasetInspectionV1,
  ApiLineageNodeV1,
  ApiPreviewV1,
  ApiVersionsV1,
  LogicalSchemaV1,
  LogicalType,
  WireValue,
} from "./generated/contracts";
import type { Workspace, WorkspaceState } from "./workspace";
import { Button, InspectorNotice } from "./components";
import { decode } from "./api/client";
import { Icon } from "./Icons";
import { PreviewGrid } from "./PreviewGrid";

function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
export function field(value: unknown, name: string): string | undefined {
  const item = object(value)?.[name];
  return typeof item === "string" ? item : undefined;
}
function nested(value: unknown, ...names: string[]): unknown {
  return names.reduce<unknown>((item, name) => object(item)?.[name], value);
}
export function schemaOf(value: unknown): LogicalSchemaV1 | null {
  try {
    return decode("LogicalSchemaV1", value);
  } catch {
    return null;
  }
}
function manifestCounts(metadata: unknown) {
  const files = nested(metadata, "manifest", "files");
  if (!Array.isArray(files))
    return { rows: undefined, bytes: undefined, files: undefined };
  const sum = (name: string): string | undefined => {
    const values = files.map((item) => field(item, name));
    return values.every(
      (value) => value !== undefined && /^(0|[1-9][0-9]*)$/.test(value),
    )
      ? values
          .reduce((total, value) => total + BigInt(value ?? "0"), 0n)
          .toString()
      : undefined;
  };
  return {
    rows: sum("row_count"),
    bytes: sum("byte_length"),
    files: String(files.length),
  };
}
export function versionMetadata(
  dataset: ApiDatasetV1,
  versions: ApiVersionsV1 | null,
  pinned?: string,
  inspection?: ApiDatasetInspectionV1 | null,
) {
  const headVersion = field(dataset.head, "version");
  const suggested = !pinned && !headVersion ? inspection?.suggested_head : null;
  const version = pinned ?? headVersion ?? suggested?.version;
  const entry = versions?.entries.find((item) => item.version === version);
  const metadata = entry?.origin === "external" ? entry.metadata : null;
  const physical = manifestCounts(metadata);
  const schema = schemaOf(
    entry?.origin === "local"
      ? entry.schema
      : (nested(metadata, "manifest", "logical_schema") ??
          nested(metadata, "schema") ??
          nested(dataset.head, "schema") ??
          suggested?.schema),
  );
  return {
    version,
    schema,
    rows:
      entry?.origin === "local"
        ? entry.row_count
        : (field(metadata, "row_count") ??
          physical.rows ??
          (!pinned ? field(dataset.head, "row_count") : undefined) ??
          suggested?.row_count),
    bytes:
      entry?.origin === "local"
        ? entry.byte_count
        : (field(metadata, "byte_count") ??
          physical.bytes ??
          (!pinned ? field(dataset.head, "byte_count") : undefined) ??
          suggested?.byte_count),
    files:
      entry?.origin === "local"
        ? entry.file_count
        : (field(metadata, "file_count") ??
          physical.files ??
          suggested?.file_count),
    published:
      entry?.origin === "local"
        ? entry.published_at_us
        : (field(metadata, "published_us") ??
          (!pinned ? field(dataset.head, "published_at_us") : undefined) ??
          suggested?.published_us),
    source:
      entry?.origin === "local"
        ? entry.source
        : (field(nested(metadata, "source"), "id") ??
          (!pinned ? field(dataset.head, "source") : undefined)),
    originBranch:
      field(nested(metadata, "origin_branch"), "name") ?? suggested?.branch,
    attempt: entry?.origin === "local" ? entry.attempt : undefined,
  };
}
function date(us?: string): string {
  if (!us) return "Unavailable";
  const stamp = Number(us) / 1000;
  if (!Number.isFinite(stamp)) return "Unavailable";
  const value = new Date(stamp);
  if (Number.isNaN(value.getTime())) return "Unavailable";
  const two = (part: number) => String(part).padStart(2, "0");
  return `${value.getFullYear()}-${two(value.getMonth() + 1)}-${two(value.getDate())} ${two(value.getHours())}:${two(value.getMinutes())}`;
}
function display(value?: string): string {
  return value ?? "Unknown";
}
function typeName(type: LogicalType): string {
  if (type.type === "decimal")
    return `decimal(${type.precision}, ${type.scale})`;
  if (type.type === "timestamp")
    return `timestamp ${type.unit}${type.timezone ? ` · ${type.timezone}` : ""}`;
  if (type.type === "list")
    return `list<${typeName(type.element.logical_type)}>`;
  if (type.type === "struct") return `struct (${type.fields.length} fields)`;
  return type.type;
}
function Detail({ name, value }: { name: string; value: string }) {
  return (
    <div className="property-row">
      <dt>{name}</dt>
      <dd>{value}</dd>
    </div>
  );
}
function Columns({ schema }: { schema: LogicalSchemaV1 | null }) {
  if (!schema)
    return (
      <p className="muted">Schema metadata is unavailable for this version.</p>
    );
  return (
    <div className="column-list" aria-label="Dataset columns">
      {schema.fields.map((column) => (
        <div className="column-item" key={column.name}>
          <strong>{column.name}</strong>
          <span>{typeName(column.logical_type)}</span>
          <small>{column.nullable ? "Nullable" : "Required"}</small>
        </div>
      ))}
    </div>
  );
}
export function PropertiesInspector({
  state,
  selected,
}: {
  state: WorkspaceState;
  selected: readonly ApiLineageNodeV1[];
}) {
  const [section, setSection] = useState<"About" | "Columns">("About");
  const ready = state.kind === "ready" ? state.value : null;
  const dataset = ready?.dataset;
  const pinned = state.selection.version;
  const meta = dataset
    ? versionMetadata(
        dataset,
        ready?.versions ?? null,
        pinned,
        ready?.inspection,
      )
    : null;
  if (selected.length !== 1) return <InspectorNotice />;
  if (!dataset || !ready)
    return (
      <section className="properties-body">
        <p className="muted">
          Select a dataset on the graph or in the catalogue.
        </p>
      </section>
    );
  const producerPath = field(dataset.producer, "path");
  const graphNode = selected.find(
    (node) =>
      node.identity === `dataset:${dataset.workspace_id}:${dataset.dataset_id}`,
  );
  const engine =
    graphNode?.resource_type === "sql_transform"
      ? "SQL"
      : dataset.kind === "transform"
        ? "Polars"
        : "Unavailable";
  const type =
    dataset.kind === "transform"
      ? `${engine} transform`
      : dataset.origin === "external"
        ? "External dataset"
        : `${dataset.kind[0]?.toUpperCase()}${dataset.kind.slice(1)} dataset`;
  return (
    <section className="properties-body" aria-label="Selected dataset context">
      <div className="property-title">
        <span className="property-type-icon" aria-hidden="true">
          {dataset.origin === "external"
            ? "↗"
            : dataset.kind === "transform"
              ? "ƒ"
              : "▦"}
        </span>
        <div>
          <h3>{dataset.path}</h3>
          <p>{type}</p>
        </div>
      </div>
      <div
        className="property-tabs"
        role="tablist"
        aria-label="Dataset properties"
      >
        {(["About", "Columns"] as const).map((name) => (
          <button
            key={name}
            role="tab"
            aria-selected={section === name}
            onClick={() => setSection(name)}
          >
            {name}
          </button>
        ))}
      </div>
      {section === "Columns" ? (
        <Columns schema={meta?.schema ?? null} />
      ) : (
        <>
          <p className="property-description">Description unavailable</p>
          <h4>Details</h4>
          <dl>
            <Detail name="Logical path" value={dataset.path} />
            <Detail name="Dataset ID" value={dataset.dataset_id} />
            <Detail
              name="Origin"
              value={
                dataset.origin === "external"
                  ? "External provider"
                  : "Local workspace"
              }
            />
            <Detail name="Type" value={type} />
            <Detail
              name="Producer"
              value={
                producerPath
                  ? `${producerPath}${field(dataset.producer, "line") ? `:${field(dataset.producer, "line")}` : ""}`
                  : "Unavailable"
              }
            />
            <Detail
              name="Created"
              value={date(ready.inspection?.created_us ?? undefined)}
            />
            <Detail name="Published" value={date(meta?.published)} />
            <Detail name="Rows" value={display(meta?.rows)} />
            <Detail name="Files" value={display(meta?.files)} />
            <Detail name="Physical bytes" value={display(meta?.bytes)} />
          </dl>
          <h4>Connections</h4>
          <dl>
            <Detail
              name="Direct inputs"
              value={graphNode?.parent_count ?? "Unknown"}
            />
            <Detail
              name="Direct consumers"
              value={graphNode?.child_count ?? "Unknown"}
            />
          </dl>
          <p className="muted">
            Explore the lineage graph for connection names and roles.
          </p>
          <h4>Data health</h4>
          <p className="muted">
            Output checks unavailable · Latest attempt unavailable
          </p>
        </>
      )}
    </section>
  );
}

function wire(value: WireValue | null): string {
  if (value === null) return "Unavailable";
  if (value.type === "null") return "NULL";
  if (value.type === "list") return `[${value.values.map(wire).join(", ")}]`;
  if (value.type === "struct")
    return `{${value.fields.map((item) => `${item.name}: ${wire(item.value)}`).join(", ")}}`;
  if (value.type === "bool") return String(value.value);
  if (value.type === "timestamp")
    return `${value.value} (${value.unit}${value.timezone ? `, ${value.timezone}` : ""})`;
  if (value.type === "decimal")
    return `${value.value} (decimal ${value.precision}, ${value.scale})`;
  if (value.type === "binary") return `Binary · ${value.value}`;
  return value.value;
}
export function PreviewInspector({
  workspace,
  state,
  dark,
}: {
  workspace: Workspace;
  state: WorkspaceState;
  dark: boolean;
}) {
  const ready = state.kind === "ready" ? state.value : null;
  const dataset = ready?.dataset;
  const meta = dataset
    ? versionMetadata(
        dataset,
        ready?.versions ?? null,
        state.selection.version,
        ready?.inspection,
      )
    : null;
  const version = meta?.version;
  const schema = meta?.schema;
  const identity = JSON.stringify([
    ready?.context.fingerprint,
    dataset?.dataset_id,
    version,
    schema?.fields.map((item) => item.name),
  ]);
  return (
    <PreviewContent
      key={identity}
      workspace={workspace}
      state={state}
      dark={dark}
      dataset={dataset ?? null}
      meta={meta}
      version={version}
      schema={schema ?? null}
    />
  );
}
function PreviewContent({
  workspace,
  state,
  dark,
  dataset,
  meta,
  version,
  schema,
}: {
  workspace: Workspace;
  state: WorkspaceState;
  dark: boolean;
  dataset: ApiDatasetV1 | null;
  meta: ReturnType<typeof versionMetadata> | null;
  version: string | undefined;
  schema: LogicalSchemaV1 | null;
}) {
  const ready = state.kind === "ready" ? state.value : null;
  const columns = useMemo(
    () => schema?.fields.map((column) => column.name) ?? [],
    [schema],
  );
  const [cursor, setCursor] = useState<string | null>(null);
  const [result, setResult] = useState<{
    key: string;
    page: ApiPreviewV1;
  } | null>(null);
  const [message, setMessage] = useState("");
  const [retry, setRetry] = useState(0);
  const key = JSON.stringify([
    ready?.context.fingerprint,
    dataset?.workspace_id,
    dataset?.dataset_id,
    version,
    columns,
  ]);
  useEffect(() => {
    if (!dataset || !version || !columns.length || columns.length > 128) return;
    const request = new AbortController();
    void workspace
      .preview(
        {
          dataset: dataset.dataset_id,
          origin_workspace: dataset.workspace_id,
          version,
          columns,
          rows: 100,
          ...(cursor ? { cursor } : {}),
        },
        request.signal,
      )
      .then(
        (page) => {
          if (request.signal.aborted) return;
          setMessage("");
          setResult((old) => ({
            key,
            page: {
              ...page,
              rows:
                cursor && old?.key === key
                  ? [...old.page.rows, ...page.rows].slice(0, 1000)
                  : page.rows.slice(0, 1000),
            },
          }));
        },
        (error: unknown) => {
          if (!request.signal.aborted)
            setMessage(
              error instanceof Error ? error.message : "Preview failed.",
            );
        },
      );
    return () => request.abort();
  }, [workspace, key, cursor, dataset, version, columns, retry]);
  const page = result?.key === key ? result.page : null;
  if (!dataset)
    return (
      <div className="preview-empty">
        Select a dataset to preview an exact published version.
      </div>
    );
  const resolved =
    page?.resolved_branch ??
    field(dataset.head, "resolved_branch") ??
    meta?.originBranch;
  const published = page?.published_us ?? meta?.published;
  return (
    <div className="preview-inspector">
      {!state.selection.version &&
        resolved &&
        resolved !== state.selection.branch && (
          <div className="preview-fallback-banner" role="status">
            <Icon name="branch" />
            Could not find data on current branch, so showing data from branch{" "}
            <strong>{resolved}</strong>
          </div>
        )}
      <div className="preview-toolbar">
        <h2>{dataset.path}</h2>
        <div className="preview-toolbar-details">
          {state.selection.version && (
            <span title={state.selection.version}>
              Version <strong>{state.selection.version.slice(0, 8)}</strong>
              {resolved ? ` · Branch ${resolved}` : ""}
            </span>
          )}
          {published && (
            <span>
              Published <strong>{date(published)}</strong>
            </span>
          )}
          {page && (
            <span>
              Showing {page.rows.length}
              {page.next_cursor && page.rows.length < 1000 ? "+" : ""}
              {meta?.rows !== undefined ? ` of ${meta.rows}` : ""} rows
              {page.next_cursor && page.rows.length === 1000
                ? " (preview limit)"
                : ""}
            </span>
          )}
          {schema && <span>{schema.fields.length} columns</span>}
        </div>
      </div>
      {!version ? (
        <p className="preview-empty">No published version is available.</p>
      ) : !schema ? (
        <p className="preview-empty">
          Schema metadata is unavailable for this version.
        </p>
      ) : columns.length > 128 ? (
        <p className="preview-empty">
          This dataset has more than the 128 columns supported by one preview
          request.
        </p>
      ) : !columns.length ? (
        <p className="preview-empty">
          This version has no previewable columns.
        </p>
      ) : (
        <div className="preview-table-wrap">
          {message && (
            <div role="alert" className="preview-error">
              {message}{" "}
              <Button
                onClick={() => {
                  setMessage("");
                  setResult(null);
                  setCursor(null);
                  setRetry((value) => value + 1);
                }}
              >
                Restart preview
              </Button>
            </div>
          )}
          {!page && !message && (
            <p role="status" className="preview-empty">
              Loading bounded preview…
            </p>
          )}
          {page && (
            <>
              <PreviewGrid
                page={page}
                path={dataset.path}
                dark={dark}
                formatCell={wire}
                formatType={(index) => {
                  const column = page.schema.fields[index];
                  return column
                    ? `${typeName(column.logical_type)}${column.nullable ? " · nullable" : ""}`
                    : "";
                }}
                onError={setMessage}
              />
              {page.rows.length === 0 && (
                <p className="preview-empty">This version contains no rows.</p>
              )}
              {page.next_cursor && page.rows.length < 1000 && (
                <div className="preview-load-more">
                  <Button onClick={() => setCursor(page.next_cursor)}>
                    Load next 100 rows
                  </Button>
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
