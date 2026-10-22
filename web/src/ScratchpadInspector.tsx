import { useEffect, useRef, useState } from "react";
import type {
  ApiContextV1,
  ApiQueryV1,
  ApiQueryResultsV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { decode } from "./api/client";
import { same } from "./api/validate";
import { Button, DatasetHeader } from "./components";
import { Icon } from "./Icons";
import { PreviewCriteriaPills } from "./PreviewCriteriaPills";
import {
  emptyPreviewCriteria,
  scratchpadPreviewQuery,
  type PreviewCriteria,
} from "./preview-query";
import { PreviewGrid } from "./PreviewGrid";
import { tabSeparatedText } from "./tabular-text";
import { field, typeName, versionMetadata, wire } from "./inspectors";
import { readableDuration } from "./execution-model";
import { Empty, Fallback, message, type Ready } from "./execution-ui";
import { SqlEditor } from "./SqlEditor";
import { bindCatalogueSql, type BoundSql } from "./scratchpad-bindings";
import { quoteSqlName } from "./sql-catalogue";

type Owned = {
  closed: boolean;
  running: boolean;
  cancelRequested: boolean;
  polling: boolean;
  pollAgain: boolean;
  downloading: boolean;
  paging: boolean;
  downloadUrl?: string;
  abort: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  query?: { id: string; context: ApiContextV1 };
};
const terminal = (job: ApiQueryV1) =>
  !["QUEUED", "RUNNING"].includes(job.state);

export function ScratchpadInspector({
  workspace,
  state,
  dark,
}: {
  workspace: Workspace;
  state: Ready;
  dark: boolean;
}) {
  const dataset = state.value.dataset;
  const meta = dataset
    ? versionMetadata(
        dataset,
        state.value.versions,
        state.selection.version,
        state.value.inspection,
      )
    : null;
  if (!dataset)
    return <Empty>Select a dataset to query its published version.</Empty>;
  return (
    <Scratchpad
      key={`${state.selection.branch}:${dataset.workspace_id}:${dataset.dataset_id}:${meta?.version ?? "unpublished"}`}
      workspace={workspace}
      state={state}
      dark={dark}
      version={meta?.version}
      resolved={
        state.selection.version
          ? undefined
          : (meta?.originBranch ?? field(dataset.head, "resolved_branch"))
      }
    />
  );
}

function Scratchpad({
  workspace,
  state,
  dark,
  version,
  resolved,
}: {
  workspace: Workspace;
  state: Ready;
  dark: boolean;
  version: string | undefined;
  resolved: string | undefined;
}) {
  const dataset = state.value.dataset;
  const [criteria, setCriteria] =
    useState<PreviewCriteria>(emptyPreviewCriteria);
  const submittedSql = useRef("");
  const submittedBindings = useRef<BoundSql | undefined>(undefined);
  const [catalogueFallbacks, setCatalogueFallbacks] = useState<
    readonly string[]
  >([]);
  const [previousRows, setPreviousRows] = useState(false);
  const [sql, setSql] = useState(
    () => `SELECT * FROM ${quoteSqlName(dataset?.path ?? "dataset")}`,
  );
  const [job, setJob] = useState<ApiQueryV1>();
  const [result, setResult] = useState<ApiQueryResultsV1>();
  const [busy, setBusy] = useState(false);
  const [canceling, setCanceling] = useState(false);
  const [error, setError] = useState("");
  const [readFailed, setReadFailed] = useState(false);
  const [moreBusy, setMoreBusy] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const owned = useRef<Owned | null>(null);
  useEffect(() => {
    const lifetime: Owned = {
      closed: false,
      running: false,
      cancelRequested: false,
      polling: false,
      pollAgain: false,
      downloading: false,
      paging: false,
      abort: new AbortController(),
    };
    owned.current = lifetime;
    return () => {
      lifetime.closed = true;
      lifetime.abort.abort();
      clearTimeout(lifetime.timer);
      if (lifetime.downloadUrl) URL.revokeObjectURL(lifetime.downloadUrl);
      if (lifetime.query && lifetime.running) {
        const { id, context } = lifetime.query;
        // The server still enforces its deadline if the connection is lost.
        void workspace.client
          .cancelQuery(id, context, new AbortController().signal)
          .catch(() => undefined);
      }
    };
  }, [workspace]);

  async function page(lifetime: Owned, offset = 0) {
    const query = lifetime.query;
    if (!query) throw new Error("The query receipt is unavailable.");
    const response = await workspace.client.read(
      "ApiQueryResultsV1",
      `/api/v1/queries/${query.id}/results`,
      { branch: query.context.branch, offset: String(offset), limit: "500" },
      lifetime.abort.signal,
      query.context,
    );
    if (
      response.data.id !== query.id ||
      response.data.rows.length > 500 ||
      response.data.rows.some(
        (row) => row.length !== response.data.schema.fields.length,
      ) ||
      (response.data.next_offset !== null &&
        (response.data.next_offset !== offset + response.data.rows.length ||
          response.data.next_offset <= offset ||
          response.data.next_offset >= 1000))
    )
      throw new Error("The result page does not match this query.");
    return response.data;
  }
  async function poll(lifetime: Owned) {
    if (!lifetime.query || lifetime.closed) return;
    if (lifetime.polling) {
      lifetime.pollAgain = true;
      return;
    }
    lifetime.polling = true;
    const query = lifetime.query;
    let finished = false;
    try {
      const { id, context } = query;
      const response = await workspace.client.read(
        "ApiQueryV1",
        `/api/v1/queries/${id}`,
        { branch: context.branch },
        lifetime.abort.signal,
        context,
      );
      if (lifetime.closed || lifetime.query !== query) return;
      if (response.data.id !== id)
        throw new Error("Query status does not match its receipt.");
      setJob(response.data);
      setReadFailed(false);
      setError("");
      if (terminal(response.data)) {
        finished = true;
        if (response.data.state === "SUCCEEDED") {
          const first = await page(lifetime);
          if (!lifetime.closed && lifetime.query === query) {
            setResult(first);
            setPreviousRows(false);
          }
        } else if (response.data.state !== "CANCELED")
          setError(
            response.data.error ??
              `Query ${response.data.state.toLowerCase()}.`,
          );
        if (!lifetime.closed) {
          lifetime.running = false;
          setBusy(false);
          setCanceling(false);
        }
      } else {
        lifetime.timer = setTimeout(() => void poll(lifetime), 500);
      }
    } catch (failure) {
      if (!lifetime.closed) {
        setError(message(failure));
        setReadFailed(true);
        if (finished) {
          lifetime.running = false;
          setBusy(false);
          setCanceling(false);
        }
      }
    } finally {
      lifetime.polling = false;
      if (lifetime.pollAgain && !lifetime.closed) {
        lifetime.pollAgain = false;
        clearTimeout(lifetime.timer);
        if (!finished)
          lifetime.timer = setTimeout(() => void poll(lifetime), 0);
      }
    }
  }
  async function run(nextCriteria?: PreviewCriteria) {
    const lifetime = owned.current;
    if (
      !dataset ||
      !version ||
      !lifetime ||
      lifetime.running ||
      lifetime.downloading ||
      (!nextCriteria && !sql.trim())
    )
      return;
    if (lifetime.downloadUrl) {
      URL.revokeObjectURL(lifetime.downloadUrl);
      delete lifetime.downloadUrl;
    }
    const sourceSql = nextCriteria ? submittedSql.current : sql;
    const next = nextCriteria ?? emptyPreviewCriteria;
    submittedSql.current = sourceSql;
    setCriteria(next);
    setPreviousRows(!!nextCriteria && !!result);
    lifetime.running = true;
    lifetime.cancelRequested = false;
    delete lifetime.query;
    setBusy(true);
    setCanceling(false);
    setError("");
    setReadFailed(false);
    setMoreBusy(false);
    setJob(undefined);
    if (!nextCriteria) {
      setResult(undefined);
      setCatalogueFallbacks([]);
    }
    try {
      // Resolve a branch-only command context: the query API binds data versions
      // explicitly in its body, independently of the displayed fallback policy.
      const start = new AbortController();
      const context = (
        await workspace.client.read(
          "ApiContextV1",
          "/api/v1/context",
          { branch: state.selection.branch },
          start.signal,
        )
      ).data;
      if (lifetime.closed) return;
      if (
        context.workspace !== state.value.context.workspace ||
        context.branch !== state.selection.branch
      )
        throw new Error("The query workspace or branch changed.");
      let bound = nextCriteria ? submittedBindings.current : undefined;
      if (!bound) {
        const lookup = {
          branch: state.selection.branch,
          ...(state.selection.fallback
            ? { fallback: JSON.stringify(state.selection.fallback) }
            : {}),
        };
        const lookupContext = state.selection.fallback
          ? (
              await workspace.client.read(
                "ApiContextV1",
                "/api/v1/context",
                lookup,
                lifetime.abort.signal,
              )
            ).data
          : context;
        if (
          !same(
            { ...context, fingerprint: "", fallback_policy: [] },
            { ...lookupContext, fingerprint: "", fallback_policy: [] },
          )
        )
          throw new Error(
            "The catalogue changed while preparing this query. Run it again.",
          );
        bound = await bindCatalogueSql(
          workspace.client,
          sourceSql,
          {
            alias: "dataset",
            dataset: dataset.dataset_id,
            origin_workspace: dataset.workspace_id,
            version,
          },
          lookupContext,
          lookup,
          lifetime.abort.signal,
          dataset.path,
        );
        submittedBindings.current = bound;
      }
      if (lifetime.closed) return;
      if (lifetime.cancelRequested) {
        lifetime.running = false;
        setBusy(false);
        setCanceling(false);
        return;
      }
      setCatalogueFallbacks(bound.fallbacks);
      const composed =
        nextCriteria && result && (next.filters.length || next.sorts.length)
          ? scratchpadPreviewQuery(bound.sql, result.schema, next)
          : { sql: bound.sql, parameters: [] };
      const request = decode("ApiQueryRequestV1", {
        ...composed,
        bindings: bound.bindings,
      });
      // Keep acceptance observable until its owned ID arrives, even on unmount.
      // Aborting acceptance could lose a successfully created helper's receipt.
      const accepted = await workspace.client.startQuery(
        request,
        context,
        crypto.randomUUID(),
        start.signal,
      );
      lifetime.query = { id: accepted.id, context };
      if (lifetime.closed || lifetime.cancelRequested)
        await workspace.client.cancelQuery(
          accepted.id,
          context,
          new AbortController().signal,
        );
      if (lifetime.closed) return;
      setJob(accepted);
      await poll(lifetime);
    } catch (failure) {
      if (!lifetime.closed) {
        lifetime.running = false;
        setBusy(false);
        setCanceling(false);
        setError(message(failure));
      }
    }
  }
  async function cancel() {
    const lifetime = owned.current;
    if (!lifetime || !lifetime.running || lifetime.cancelRequested) return;
    lifetime.cancelRequested = true;
    setCanceling(true);
    if (!lifetime.query) return;
    try {
      await workspace.client.cancelQuery(
        lifetime.query.id,
        lifetime.query.context,
        new AbortController().signal,
      );
      if (!lifetime.closed) {
        clearTimeout(lifetime.timer);
        await poll(lifetime);
      }
    } catch (failure) {
      if (!lifetime.closed) {
        lifetime.cancelRequested = false;
        setCanceling(false);
        setError(message(failure));
      }
    }
  }
  async function more() {
    const lifetime = owned.current;
    if (
      !lifetime ||
      !result ||
      result.next_offset === null ||
      lifetime.paging ||
      lifetime.running ||
      previousRows ||
      error ||
      lifetime.downloading
    )
      return;
    const query = lifetime.query;
    lifetime.paging = true;
    setMoreBusy(true);
    try {
      const next = await page(lifetime, result.next_offset);
      if (lifetime.closed || lifetime.query !== query) return;
      if (
        !same(next.schema, result.schema) ||
        result.rows.length + next.rows.length > 1000
      )
        throw new Error("Query result schema or row bounds changed.");
      if (!lifetime.closed)
        setResult({ ...next, rows: [...result.rows, ...next.rows] });
    } catch (failure) {
      if (!lifetime.closed && lifetime.query === query)
        setError(message(failure));
    } finally {
      lifetime.paging = false;
      if (!lifetime.closed && lifetime.query === query) setMoreBusy(false);
    }
  }
  async function download() {
    const lifetime = owned.current;
    const query = lifetime?.query;
    if (
      !lifetime ||
      !query ||
      !result ||
      !job ||
      previousRows ||
      job.state !== "SUCCEEDED" ||
      lifetime.closed ||
      lifetime.downloading ||
      moreBusy
    )
      return;
    lifetime.downloading = true;
    setDownloading(true);
    setError("");
    try {
      let rows = [...result.rows];
      let offset = result.next_offset;
      while (offset !== null) {
        const next = await page(lifetime, offset);
        if (lifetime.closed || lifetime.query !== query) return;
        if (
          !same(next.schema, result.schema) ||
          rows.length + next.rows.length > 1000
        )
          throw new Error("Query result schema or row bounds changed.");
        rows = [...rows, ...next.rows];
        offset = next.next_offset;
      }
      if (lifetime.closed || lifetime.query !== query) return;
      if (BigInt(rows.length) !== BigInt(job.rows))
        throw new Error(
          "The complete query result is unavailable for download.",
        );
      const text = tabSeparatedText(
        result.schema.fields.map((field) => field.name),
        rows.map((row) => row.map(wire)),
      );
      if (lifetime.downloadUrl) URL.revokeObjectURL(lifetime.downloadUrl);
      const url = URL.createObjectURL(
        new Blob([text], {
          type: "text/tab-separated-values;charset=utf-8",
        }),
      );
      lifetime.downloadUrl = url;
      const link = document.createElement("a");
      link.href = url;
      link.download = "sql-results.tsv";
      document.body.append(link);
      try {
        link.click();
      } finally {
        link.remove();
      }
    } catch (failure) {
      if (!lifetime.closed && lifetime.query === query)
        setError(message(failure));
    } finally {
      lifetime.downloading = false;
      if (!lifetime.closed && lifetime.query === query) setDownloading(false);
    }
  }
  return (
    <div className="scratchpad-inspector">
      <Fallback branch={resolved} requested={state.selection.branch} />
      {!!catalogueFallbacks.length && (
        <div className="scratchpad-catalogue-fallback" role="status">
          Using published data: {catalogueFallbacks.join("; ")}.
        </div>
      )}
      <DatasetHeader path={dataset?.path} actionsAfterName>
        <Button
          className="button-primary"
          disabled={busy || downloading || !version || !sql.trim()}
          onClick={() => void run()}
        >
          <Icon name="play" /> Run query
        </Button>
        <Button disabled={!busy || canceling} onClick={() => void cancel()}>
          {canceling ? "Canceling…" : "Cancel query"}
        </Button>
        {busy && !readFailed && (!job || !terminal(job)) && (
          <span
            className="scratchpad-running"
            role="status"
            aria-label="Query running"
          >
            <Icon name="cycle" /> Running query…
          </span>
        )}
        {(criteria.filters.length > 0 || criteria.sorts.length > 0) && (
          <div className="scratchpad-preview-controls">
            <Button
              disabled={busy || downloading}
              onClick={() => void run(emptyPreviewCriteria)}
            >
              Reset preview
            </Button>
            <PreviewCriteriaPills criteria={criteria} />
          </div>
        )}
        {(job || busy) && (
          <div className="scratchpad-result-actions">
            <div
              className="scratchpad-status"
              role="status"
              aria-label="Query status"
            >
              {busy && !readFailed && (!job || !terminal(job))
                ? null
                : readFailed && job && !terminal(job)
                  ? "Query status unavailable"
                  : job
                    ? `${job.state === "SUCCEEDED" ? "" : `${job.state} · `}${job.rows} rows · ${readableDuration((BigInt(job.elapsed_ms) * 1000000n).toString())}`
                    : "Starting query…"}
              {job?.truncated && (
                <span> · Results truncated at the query limits.</span>
              )}
            </div>
            {result && job?.state === "SUCCEEDED" && (
              <Button
                title="Download all available result rows as a tab-delimited file"
                disabled={busy || moreBusy || downloading || previousRows}
                onClick={() => void download()}
              >
                <Icon name="download" />
                {downloading ? "Preparing download…" : "Download TSV"}
              </Button>
            )}
          </div>
        )}
      </DatasetHeader>
      <div className="scratchpad-layout">
        <section className="scratchpad-inputs" aria-label="SQL editor">
          <SqlEditor
            workspace={workspace}
            selectedPath={dataset?.path ?? "dataset"}
            value={sql}
            onChange={setSql}
          />
          {!version && (
            <p className="scratchpad-unavailable">
              No published version is available for this query.
            </p>
          )}
        </section>
        <section
          className="scratchpad-results"
          aria-label="SQL results"
          aria-busy={busy}
        >
          {error && (
            <p className="execution-error" role="alert">
              {error} {previousRows && "Previous rows are shown disabled. "}
              {readFailed && (
                <Button
                  onClick={() => {
                    const lifetime = owned.current;
                    if (lifetime) void poll(lifetime);
                  }}
                >
                  Retry status
                </Button>
              )}
            </p>
          )}
          {result ? (
            <>
              <PreviewGrid
                page={{
                  schema: result.schema,
                  rows: result.rows.map((row) =>
                    row.map((value) => ({ value, truncated: false })),
                  ),
                }}
                busy={busy && previousRows}
                disabled={previousRows || downloading}
                criteria={criteria}
                onCriteriaChange={(next) => void run(next)}
                onNearEnd={() => void more()}
                path="SQL query results"
                dark={dark}
                formatCell={wire}
                formatType={(index) =>
                  typeName(
                    result.schema.fields[index]?.logical_type ?? {
                      type: "string",
                    },
                  )
                }
                onError={setError}
              />
              {result.rows.length === 0 && (
                <p className="scratchpad-result-notice">
                  The query returned no rows.
                </p>
              )}
            </>
          ) : !error || busy ? (
            <div className="scratchpad-empty-results">
              <Icon name="database" />
              <p>
                {busy
                  ? "Your results will appear here when the query finishes."
                  : job?.state === "CANCELED"
                    ? "Query canceled. Run another query to see the results."
                    : "Run a SQL query in the editor to see a preview of the results."}
              </p>
            </div>
          ) : null}
        </section>
      </div>
    </div>
  );
}
