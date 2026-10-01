import { useEffect, useState } from "react";
import type {
  ApiAttemptV1,
  ApiAttemptLogsV1,
  ApiHistoryJobV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { field } from "./inspectors";
import { seconds, short, sortedHistory, timestamp } from "./execution-model";
import {
  type Ready,
  type Scope,
  type HistoryResult,
  branchSelection,
  message,
  Empty,
  Evidence,
  property,
} from "./execution-ui";
import { BuildDetail } from "./BuildReport";
export function TimelineInspector({
  workspace,
  state,
}: {
  workspace: Workspace;
  state: Ready;
}) {
  const dataset = state.value.dataset;
  const [history, setHistory] = useState<HistoryResult>();
  const [selected, setSelected] = useState<ApiHistoryJobV1>();
  const [error, setError] = useState("");
  useEffect(() => {
    if (!dataset || dataset.origin === "external") return;
    const abort = new AbortController();
    void workspace
      .scope(branchSelection(state), abort.signal)
      .then((scope) =>
        scope.read(
          "ApiDatasetHistoryV1",
          `/api/v1/datasets/${dataset.dataset_id}/history`,
          { limit: "50" },
        ),
      )
      .then(
        (page) => {
          if (!abort.signal.aborted) {
            setHistory({ entries: page.entries, next: page.next_cursor });
            setError("");
          }
        },
        (error) => {
          if (!abort.signal.aborted) setError(message(error));
        },
      );
    return () => abort.abort();
  }, [workspace, state, dataset]);
  const jobs = sortedHistory(history?.entries ?? []),
    job = selected ?? jobs[0];
  async function more() {
    if (!dataset || !history?.next) return;
    try {
      const scope = await workspace.scope(
        branchSelection(state),
        new AbortController().signal,
      );
      const page = await scope.read(
        "ApiDatasetHistoryV1",
        `/api/v1/datasets/${dataset.dataset_id}/history`,
        { cursor: history.next, limit: "50" },
      );
      setHistory({
        entries: [...history.entries, ...page.entries],
        next: page.next_cursor,
      });
    } catch (error) {
      setError(message(error));
    }
  }
  return (
    <div className="execution-inspector">
      <div className="execution-toolbar">
        <strong>{dataset?.path}</strong>
        {jobs.length > 0 && (
          <label>
            Build{" "}
            <select
              value={job?.id ?? ""}
              onChange={(event) =>
                setSelected(jobs.find((job) => job.id === event.target.value))
              }
            >
              {jobs.map((job) => (
                <option key={job.id} value={job.id}>
                  {timestamp(job.created_us)} · {job.state} · {short(job.build)}
                </option>
              ))}
            </select>
          </label>
        )}
        {history?.next && (
          <Button
            disabled={jobs.length >= 1000}
            onClick={() => {
              void more();
            }}
          >
            Load more builds
          </Button>
        )}
      </div>
      {error && (
        <p className="execution-error" role="alert">
          {error}
        </p>
      )}
      {dataset?.origin === "external" ? (
        <Empty>
          Build evidence is retained by the provider workspace and is
          unavailable here.
        </Empty>
      ) : job && dataset ? (
        <div className="execution-scroll">
          <BuildDetail
            key={job.id}
            workspace={workspace}
            state={state}
            job={job}
            dataset={dataset}
          />
        </div>
      ) : (
        !error && (
          <Empty>
            {history
              ? "No retained builds for this dataset on this branch."
              : "Loading retained builds…"}
          </Empty>
        )
      )}
    </div>
  );
}
export function AttemptDetail({
  scope,
  attempt,
}: {
  scope: Scope;
  attempt: string;
}) {
  const [detail, setDetail] = useState<ApiAttemptV1>();
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    void scope.read("ApiAttemptV1", `/api/v1/attempts/${attempt}`).then(
      (value) => {
        if (live) setDetail(value);
      },
      (error) => {
        if (live) setError(message(error));
      },
    );
    return () => {
      live = false;
    };
  }, [scope, attempt]);
  if (error)
    return (
      <p role="alert" className="execution-error">
        Attempt unavailable: {error}
      </p>
    );
  if (!detail || detail.attempt !== attempt)
    return <Empty>Loading attempt evidence…</Empty>;
  return (
    <section className="attempt-detail" aria-label="Attempt details">
      <div className="execution-toolbar">
        <strong>Attempt {short(attempt)}</strong>
        <span>{field(detail.evidence, "state")}</span>
        <span>{seconds(detail.duration_ns)}</span>
        <span title={detail.source}>
          Snapshot {short(detail.source)} · {detail.source_availability}
        </span>
      </div>
      {field(detail.evidence, "failure_class") && (
        <p className="execution-error" role="status">
          Failure: {field(detail.evidence, "failure_class")}
        </p>
      )}
      <Logs scope={scope} attempt={attempt} />
      <div className="attempt-evidence">
        <details>
          <summary>Inputs and bindings</summary>
          <Evidence>{detail.inputs}</Evidence>
        </details>
        <details>
          <summary>Captured parameters</summary>
          <Evidence>{detail.parameters}</Evidence>
        </details>
        <details>
          <summary>Output checks and process result</summary>
          <Evidence>{property(detail.evidence, "checks")}</Evidence>
          <Evidence>{property(detail.evidence, "report")}</Evidence>
        </details>
        <details>
          <summary>Recorded phases</summary>
          <Evidence>{property(detail.evidence, "phases")}</Evidence>
        </details>
      </div>
    </section>
  );
}
export function Logs({ scope, attempt }: { scope: Scope; attempt: string }) {
  const [page, setPage] = useState<ApiAttemptLogsV1>();
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    void scope
      .read("ApiAttemptLogsV1", `/api/v1/attempts/${attempt}/logs`, {
        limit: "32768",
      })
      .then(
        (value) => {
          if (live) {
            setPage(value);
            setText(value.text);
          }
        },
        (error) => {
          if (live) setError(message(error));
        },
      );
    return () => {
      live = false;
    };
  }, [scope, attempt]);
  async function load(helper: string, stream: string, offset = "0") {
    setBusy(true);
    try {
      const value = await scope.read(
        "ApiAttemptLogsV1",
        `/api/v1/attempts/${attempt}/logs`,
        { helper, stream, offset, limit: "32768" },
      );
      setPage(value);
      setText((old) =>
        (offset === "0" ? value.text : old + value.text).slice(0, 1024 * 1024),
      );
      setError("");
    } catch (error) {
      setError(message(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="attempt-logs">
      <div className="execution-toolbar">
        <strong>Logs</strong>
        {page?.available && (
          <label>
            Stream{" "}
            <select
              disabled={busy}
              value={JSON.stringify([page.helper, page.stream])}
              onChange={(event) => {
                const selected = page.streams.find(
                  (stream) =>
                    JSON.stringify([stream.helper, stream.stream]) ===
                    event.target.value,
                );
                if (selected) void load(selected.helper, selected.stream);
              }}
            >
              {page.streams.map((stream) => (
                <option
                  key={`${stream.helper}:${stream.stream}`}
                  value={JSON.stringify([stream.helper, stream.stream])}
                >
                  {stream.helper} · {stream.stream} ({stream.bytes} bytes)
                </option>
              ))}
            </select>
          </label>
        )}
        {page?.next_offset && page.helper && page.stream && (
          <Button
            disabled={busy || text.length >= 1024 * 1024}
            onClick={() => {
              if (page.helper && page.stream && page.next_offset)
                void load(page.helper, page.stream, page.next_offset);
            }}
          >
            {busy ? "Loading…" : "Load next 32 KiB"}
          </Button>
        )}
      </div>
      {error ? (
        <p role="alert" className="execution-error">
          Logs unavailable: {error}
        </p>
      ) : page ? (
        page.available ? (
          <>
            <div
              className="log-output"
              role="textbox"
              aria-readonly="true"
              aria-multiline="true"
              tabIndex={0}
              aria-label="Retained log excerpt"
            >
              {text || "No output recorded in this stream."}
            </div>
            {page.next_offset && (
              <p className="bounded-notice">
                Partial log excerpt ·{" "}
                {text.length >= 1024 * 1024
                  ? "1 MiB display limit reached."
                  : "More retained output is available."}
              </p>
            )}
          </>
        ) : (
          <Empty>No retained log streams for this attempt.</Empty>
        )
      ) : (
        <Empty>Loading bounded logs…</Empty>
      )}
    </div>
  );
}
