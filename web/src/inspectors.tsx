import { useEffect, useMemo, useState } from "react";
import type {
  ApiDatasetV1,
  ApiLineageNodeV1,
  ApiPreviewV1,
  ApiVersionsV1,
  ExecutionJsonV1,
  LogicalSchemaV1,
  LogicalType,
  WireValue,
} from "./generated/contracts";
import type { Workspace, WorkspaceState } from "./workspace";
import { Button } from "./components";
import { decode } from "./api/client";

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
) {
  const version = pinned ?? field(dataset.head, "version");
  const entry = versions?.entries.find((item) => item.version === version);
  const metadata = entry?.origin === "external" ? entry.metadata : null;
  const physical = manifestCounts(metadata);
  const schema = schemaOf(
    entry?.origin === "local"
      ? entry.schema
      : (nested(metadata, "manifest", "logical_schema") ??
          nested(metadata, "schema") ??
          nested(dataset.head, "schema")),
  );
  return {
    version,
    schema,
    rows:
      entry?.origin === "local"
        ? entry.row_count
        : (field(metadata, "row_count") ??
          physical.rows ??
          (!pinned ? field(dataset.head, "row_count") : undefined)),
    bytes:
      entry?.origin === "local"
        ? entry.byte_count
        : (field(metadata, "byte_count") ??
          physical.bytes ??
          (!pinned ? field(dataset.head, "byte_count") : undefined)),
    files:
      entry?.origin === "local"
        ? entry.file_count
        : (field(metadata, "file_count") ?? physical.files),
    published:
      entry?.origin === "local"
        ? entry.published_at_us
        : (field(metadata, "published_us") ??
          (!pinned ? field(dataset.head, "published_at_us") : undefined)),
    source:
      entry?.origin === "local"
        ? entry.source
        : (field(nested(metadata, "source"), "id") ??
          (!pinned ? field(dataset.head, "source") : undefined)),
    originBranch: field(nested(metadata, "origin_branch"), "name"),
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
function tags(value: ExecutionJsonV1): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
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
function tally(values: readonly string[]) {
  const result = new Map<string, number>();
  for (const value of values) result.set(value, (result.get(value) ?? 0) + 1);
  return [...result].sort((a, b) => b[1] - a[1]);
}
function MultiSelectionSummary({
  workspace,
  state,
  selected,
}: {
  workspace: Workspace;
  state: WorkspaceState;
  selected: readonly ApiLineageNodeV1[];
}) {
  const ready = state.kind === "ready" ? state.value : null;
  const identity = JSON.stringify([
    ready?.context.fingerprint,
    selected.map((node) => node.identity),
  ]);
  const [counts, setCounts] = useState<{
    key: string;
    rows: string;
    bytes: string;
    rowKnown: number;
    byteKnown: number;
  } | null>(null);
  useEffect(() => {
    if (!ready) return;
    const request = new AbortController();
    const selectedIds = [
      ...new Set(selected.map((node) => node.identity)),
    ].slice(0, 50);
    void (async () => {
      let rows = 0n,
        bytes = 0n,
        rowKnown = 0,
        byteKnown = 0;
      for (
        let index = 0;
        index < selectedIds.length && !request.signal.aborted;
        index += 4
      ) {
        const group = selectedIds.slice(index, index + 4);
        const details = await Promise.all(
          group.map(async (identity) => {
            const [, origin, id] = identity.split(":");
            if (!origin || !id) return null;
            if (
              ready.dataset?.dataset_id === id &&
              ready.dataset.workspace_id === origin
            ) {
              const meta = versionMetadata(
                ready.dataset,
                ready.versions,
                state.selection.version,
              );
              return { rows: meta.rows, bytes: meta.bytes };
            }
            try {
              const dataset = await workspace.read(
                "ApiDatasetV1",
                `/api/v1/datasets/${encodeURIComponent(id)}`,
                { origin_workspace: origin },
                request.signal,
              );
              return {
                rows: field(dataset.head, "row_count"),
                bytes: field(dataset.head, "byte_count"),
              };
            } catch {
              return null;
            }
          }),
        );
        for (const item of details) {
          if (item?.rows && /^(0|[1-9][0-9]*)$/.test(item.rows)) {
            rows += BigInt(item.rows);
            rowKnown++;
          }
          if (item?.bytes && /^(0|[1-9][0-9]*)$/.test(item.bytes)) {
            bytes += BigInt(item.bytes);
            byteKnown++;
          }
        }
      }
      if (!request.signal.aborted)
        setCounts({
          key: identity,
          rows: rows.toString(),
          bytes: bytes.toString(),
          rowKnown,
          byteKnown,
        });
    })();
    return () => request.abort();
  }, [workspace, ready, selected, identity, state.selection.version]);
  const current = counts?.key === identity ? counts : null;
  const unique = new Map(selected.map((node) => [node.identity, node]));
  const nodes = [...unique.values()];
  return (
    <section
      className="properties-body multi-properties"
      aria-label="Selected datasets summary"
    >
      <h3>{nodes.length} datasets selected</h3>
      <p className="muted">
        Counts describe unique datasets in the current graph selection.
      </p>
      <h4>Origins</h4>
      <dl>
        {tally(nodes.map((node) => (node.external ? "external" : "local"))).map(
          ([name, count]) => (
            <Detail key={name} name={name} value={String(count)} />
          ),
        )}
      </dl>
      <h4>Resource types</h4>
      <dl>
        {tally(
          nodes.map((node) => node.resource_type.replaceAll("_", " ")),
        ).map(([name, count]) => (
          <Detail key={name} name={name} value={String(count)} />
        ))}
      </dl>
      <h4>Publication</h4>
      <dl>
        {tally(nodes.map((node) => node.publication)).map(([name, count]) => (
          <Detail key={name} name={name} value={String(count)} />
        ))}
      </dl>
      <h4>Physical metrics</h4>
      <dl>
        <Detail
          name="Known rows"
          value={current?.rowKnown ? current.rows : "Unknown"}
        />
        <Detail
          name="Unknown row counts"
          value={String(nodes.length - (current?.rowKnown ?? 0))}
        />
        <Detail
          name="Known bytes"
          value={current?.byteKnown ? current.bytes : "Unknown"}
        />
        <Detail
          name="Unknown byte counts"
          value={String(nodes.length - (current?.byteKnown ?? 0))}
        />
      </dl>
      {nodes.length > 50 && (
        <p className="muted">
          Physical metadata is inspected for the first 50 selected datasets; the
          rest count as unknown.
        </p>
      )}
    </section>
  );
}
export function PropertiesInspector({
  workspace,
  state,
  selected,
}: {
  workspace: Workspace;
  state: WorkspaceState;
  selected: readonly ApiLineageNodeV1[];
}) {
  const [section, setSection] = useState<"About" | "Columns">("About");
  const ready = state.kind === "ready" ? state.value : null;
  const dataset = ready?.dataset;
  const pinned = state.selection.version;
  const meta = dataset
    ? versionMetadata(dataset, ready?.versions ?? null, pinned)
    : null;
  if (selected.length > 1 && ready)
    return (
      <MultiSelectionSummary
        workspace={workspace}
        state={state}
        selected={selected}
      />
    );
  if (!dataset || !ready)
    return (
      <section className="properties-body">
        <p className="muted">
          Select a dataset on the graph or in the catalogue.
        </p>
      </section>
    );
  const fallback = field(dataset.head, "resolved_branch");
  const rank = fallback ? ready.context.fallback_policy.indexOf(fallback) : -1;
  const producerPath = field(dataset.producer, "path");
  const tagNames = tags(dataset.tags);
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
          <p>
            {dataset.origin === "external"
              ? "External dataset"
              : dataset.kind === "transform"
                ? `${engine} transform`
                : `${dataset.kind[0]?.toUpperCase()}${dataset.kind.slice(1)} dataset`}
          </p>
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
                  ? `External provider · ${dataset.workspace_id}`
                  : `Local workspace · ${dataset.workspace_id}`
              }
            />
            <Detail name="Type" value={dataset.kind} />
            <Detail name="Engine" value={engine} />
            <Detail
              name="Producer"
              value={
                producerPath
                  ? `${producerPath}${field(dataset.producer, "line") ? `:${field(dataset.producer, "line")}` : ""}`
                  : "Unavailable"
              }
            />
            <Detail name="Requested branch" value={state.selection.branch} />
            <Detail
              name="Resolved head branch"
              value={
                fallback ??
                (pinned ? "Not queried for an exact version" : "Unavailable")
              }
            />
            <Detail
              name="Fallback rank"
              value={
                rank < 0
                  ? "Unavailable"
                  : rank === 0
                    ? "Requested branch"
                    : String(rank)
              }
            />
            {pinned && (
              <Detail
                name="Published version branch"
                value={meta?.originBranch ?? "Unavailable"}
              />
            )}
            <Detail
              name="Published head"
              value={
                field(dataset.head, "version") ??
                (pinned
                  ? "Not queried in historical context"
                  : dataset.origin === "external"
                    ? "Foreign head not loaded"
                    : "No published head")
              }
            />
            <Detail
              name="Exact version"
              value={pinned ?? "Not pinned — browsing retained metadata"}
            />
            <Detail
              name={
                pinned
                  ? "Producing source capture"
                  : "Retained definition source"
              }
              value={
                pinned
                  ? (meta?.source ??
                    (dataset.origin === "external"
                      ? "Foreign source metadata only"
                      : "Unavailable"))
                  : dataset.origin === "external"
                    ? "Foreign source metadata only"
                    : (ready.context.source ?? "Unavailable")
              }
            />
            {!pinned && (
              <Detail
                name="Head producing source"
                value={meta?.source ?? "Unavailable"}
              />
            )}
            <Detail name="Created" value="Unavailable" />
            <Detail
              name="Last producing attempt"
              value={meta?.attempt ?? "Unavailable"}
            />
            <Detail name="Published" value={date(meta?.published)} />
            <Detail name="Rows" value={display(meta?.rows)} />
            <Detail name="Files" value={display(meta?.files)} />
            <Detail name="Physical bytes" value={display(meta?.bytes)} />
          </dl>
          <label className="property-version">
            Version context
            <select
              value={pinned ?? ""}
              onChange={(event) =>
                void workspace.select({
                  branch: state.selection.branch,
                  dataset: dataset.dataset_id,
                  origin: dataset.workspace_id,
                  ...(event.target.value
                    ? { version: event.target.value }
                    : {}),
                  ...(state.selection.fallback
                    ? { fallback: state.selection.fallback }
                    : {}),
                })
              }
            >
              <option value="">Current retained definition</option>
              {ready.versions?.entries.map((item) => (
                <option key={item.version} value={item.version}>
                  {item.version}
                </option>
              ))}
            </select>
          </label>
          {ready.versions?.next_cursor && (
            <p className="muted">
              More versions are available in the History inspector.
            </p>
          )}
          <h4>Tags</h4>
          <p>{tagNames.length ? tagNames.join(", ") : "No tags recorded"}</p>
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
          <h4>Schedules</h4>
          <p className="muted">Scheduling is not available yet.</p>
          <p className="property-footnote">
            {dataset.origin === "external"
              ? "Foreign metadata is retained locally. Provider byte availability: not verified. Provider freshness: unknown."
              : "Metadata availability does not verify local artifact bytes or freshness. Byte availability: not verified. Freshness: unknown."}
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
}: {
  workspace: Workspace;
  state: WorkspaceState;
}) {
  const ready = state.kind === "ready" ? state.value : null;
  const dataset = ready?.dataset;
  const meta = dataset
    ? versionMetadata(dataset, ready?.versions ?? null, state.selection.version)
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
  dataset,
  meta,
  version,
  schema,
}: {
  workspace: Workspace;
  state: WorkspaceState;
  dataset: ApiDatasetV1 | null;
  meta: ReturnType<typeof versionMetadata> | null;
  version: string | undefined;
  schema: LogicalSchemaV1 | null;
}) {
  const ready = state.kind === "ready" ? state.value : null;
  const [columns, setColumns] = useState<readonly string[]>(
    () => schema?.fields.slice(0, 12).map((item) => item.name) ?? [],
  );
  const [columnSearch, setColumnSearch] = useState("");
  const [cursor, setCursor] = useState<string | null>(null);
  const [result, setResult] = useState<{
    key: string;
    page: ApiPreviewV1;
  } | null>(null);
  const [message, setMessage] = useState("");
  const [copy, setCopy] = useState<{ row: number; column: number } | null>(
    null,
  );
  const [retry, setRetry] = useState(0);
  const key = JSON.stringify([
    ready?.context.fingerprint,
    dataset?.workspace_id,
    dataset?.dataset_id,
    version,
    columns,
  ]);
  useEffect(() => {
    if (!dataset || !version || !columns.length) return;
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
                  ? [...old.page.rows, ...page.rows]
                  : page.rows,
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
  const shown = useMemo(
    () =>
      schema?.fields.filter((item) =>
        item.name.toLowerCase().includes(columnSearch.toLowerCase()),
      ) ?? [],
    [schema, columnSearch],
  );
  const toggle = (name: string) => {
    setColumns((old) =>
      old.includes(name) ? old.filter((item) => item !== name) : [...old, name],
    );
    setCursor(null);
    setResult(null);
    setCopy(null);
  };
  if (!dataset)
    return (
      <div className="preview-empty">
        Select a dataset to preview an exact published version.
      </div>
    );
  return (
    <div className="preview-inspector">
      <div className="preview-toolbar">
        <div>
          <h2>{dataset.path}</h2>
          <p>Preview · {version ?? "No version selected"}</p>
        </div>
        <span>
          {page
            ? `Showing ${page.rows.length}${page.next_cursor ? "+" : ""}${meta?.rows !== undefined ? ` of ${meta.rows}` : ""} rows`
            : "Physical row preview"}
        </span>
      </div>
      <div className="preview-context">
        Requested branch <strong>{state.selection.branch}</strong>
        <span>·</span> Resolved branch{" "}
        <strong>
          {page?.resolved_branch ??
            field(dataset.head, "resolved_branch") ??
            "Unknown"}
        </strong>
        <span>·</span> Published{" "}
        <strong>{date(page?.published_us ?? meta?.published)}</strong>
        <span>·</span> Source{" "}
        <strong>{page?.source ?? meta?.source ?? "Unknown"}</strong>
      </div>
      {!version ? (
        <p className="muted">
          Choose a retained version in Properties to enable preview.
        </p>
      ) : !schema ? (
        <p className="muted">
          Schema metadata is unavailable for this version. Select a version with
          a known schema.
        </p>
      ) : (
        <>
          <div className="preview-column-tools">
            <label>
              Search columns
              <input
                type="search"
                value={columnSearch}
                onChange={(event) => setColumnSearch(event.target.value)}
                placeholder="Search columns…"
              />
            </label>
            <details>
              <summary>
                Columns ({columns.length} of {schema.fields.length})
              </summary>
              <div className="preview-column-menu">
                {shown.map((item) => (
                  <label key={item.name}>
                    <input
                      type="checkbox"
                      checked={columns.includes(item.name)}
                      onChange={() => toggle(item.name)}
                    />
                    {item.name}
                    <small>{typeName(item.logical_type)}</small>
                  </label>
                ))}
              </div>
            </details>
            <span>Physical order · exact version</span>
          </div>
          {!columns.length ? (
            <p className="muted">Select at least one column to preview rows.</p>
          ) : (
            <>
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
                <p role="status" className="muted">
                  Loading bounded preview…
                </p>
              )}
              {page && (
                <>
                  <div className="preview-table-wrap">
                    <table className="preview-table">
                      <thead>
                        <tr>
                          <th scope="col">#</th>
                          {page.schema.fields.map((item) => (
                            <th key={item.name} scope="col">
                              <strong>{item.name}</strong>
                              <small>
                                {typeName(item.logical_type)}
                                {item.nullable ? " · nullable" : ""}
                              </small>
                            </th>
                          ))}
                        </tr>
                      </thead>
                      <tbody>
                        {page.rows.map((row, rowIndex) => (
                          <tr key={rowIndex}>
                            <th scope="row">{rowIndex + 1}</th>
                            {row.map((cell, columnIndex) => (
                              <td
                                key={columnIndex}
                                className={
                                  cell.truncated
                                    ? "truncated-cell"
                                    : cell.value?.type === "null"
                                      ? "null-cell"
                                      : ""
                                }
                              >
                                <button
                                  title="Select cell to copy"
                                  onClick={() =>
                                    setCopy({
                                      row: rowIndex,
                                      column: columnIndex,
                                    })
                                  }
                                >
                                  {cell.truncated
                                    ? "Truncated · value unavailable"
                                    : wire(cell.value)}
                                </button>
                              </td>
                            ))}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                  {page.rows.length === 0 && (
                    <p className="muted">This version contains no rows.</p>
                  )}
                  <div className="preview-footer">
                    {page.next_cursor && (
                      <Button onClick={() => setCursor(page.next_cursor)}>
                        Load next 100 rows
                      </Button>
                    )}
                    {copy && (
                      <span>
                        Selected row {copy.row + 1}, column{" "}
                        {page.schema.fields[copy.column]?.name}. Copying may
                        expose sensitive data.{" "}
                        <Button
                          onClick={() => {
                            const cell = page.rows[copy.row]?.[copy.column];
                            if (cell && !cell.truncated)
                              void navigator.clipboard.writeText(
                                wire(cell.value),
                              );
                            setCopy(null);
                          }}
                        >
                          Copy selected cell
                        </Button>
                      </span>
                    )}
                  </div>
                </>
              )}
            </>
          )}
        </>
      )}
      <p className="preview-caveat">
        A preview checks only projected bytes. It does not verify full
        integrity, data health or freshness. External data stays with its
        provider.
      </p>
    </div>
  );
}
