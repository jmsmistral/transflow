import { useEffect, useState } from "react";
import type {
  ApiAttemptV1,
  ApiAttemptLogsV1,
  ApiDatasetV1,
  ApiExecutionTimelineV1,
  ApiHistoryJobV1,
  ExecutionJsonV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { field } from "./inspectors";
import {
  position,
  seconds,
  short,
  sortedHistory,
  timestamp,
} from "./execution-model";
import {
  type Ready,
  type Scope,
  type HistoryResult,
  branchSelection,
  message,
  Empty,
  Metric,
  Status,
  Evidence,
  property,
  items,
} from "./execution-ui";
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
export function BuildDetail({
  workspace,
  state,
  job,
  dataset,
}: {
  workspace: Workspace;
  state: Ready;
  job: ApiHistoryJobV1;
  dataset: ApiDatasetV1;
}) {
  const [result, setResult] = useState<{
    timeline: ApiExecutionTimelineV1;
    report: ExecutionJsonV1;
    scope: Scope;
  }>();
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState<string>();
  const [critical, setCritical] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [cancelMessage, setCancelMessage] = useState("");
  const [canceling, setCanceling] = useState(false);
  const [cancelKey] = useState(() => crypto.randomUUID());
  useEffect(() => {
    const abort = new AbortController();
    void (async () => {
      const scope = await workspace.scope(
        { branch: state.selection.branch, plan: job.plan },
        abort.signal,
      );
      const [timeline, report] = await Promise.all([
        scope.read(
          "ApiExecutionTimelineV1",
          `/api/v1/builds/${job.build}/timeline`,
        ),
        scope.read("ExecutionJsonV1", `/api/v1/builds/${job.build}`),
      ]);
      if (
        timeline.build !== job.build ||
        timeline.plan !== job.plan ||
        timeline.source !== job.source
      )
        throw new Error("Timeline does not match the selected retained plan.");
      return { timeline, report, scope };
    })().then(
      (value) => {
        if (!abort.signal.aborted) {
          setResult(value);
          setError("");
        }
      },
      (error) => {
        if (!abort.signal.aborted) setError(message(error));
      },
    );
    return () => abort.abort();
  }, [workspace, state, job, refresh]);
  async function cancel() {
    if (!result) return;
    setCanceling(true);
    try {
      const response = await result.scope.cancel(job.build, cancelKey);
      setCancelMessage(
        response.disposition === "TooLate"
          ? "This build has already finished."
          : "Cancellation requested; running attempts stop at their next safe boundary.",
      );
      setRefresh((n) => n + 1);
    } catch (error) {
      setError(message(error));
    } finally {
      setCanceling(false);
    }
  }
  if (error)
    return (
      <p role="alert" className="execution-error">
        Retained build unavailable: {error}{" "}
        <Button onClick={() => setRefresh((n) => n + 1)}>Refresh</Button>
      </p>
    );
  if (!result || result.timeline.build !== job.build)
    return <Empty>Loading plan-bound build evidence…</Empty>;
  const { timeline, report } = result;
  const active = !["SUCCEEDED", "FAILED", "CANCELED", "INTERRUPTED"].includes(
    timeline.state,
  );
  const plan = property(report, "plan"),
    writes = items(property(plan, "writes"));
  const labels = new Map(
    writes.map((write) => [
      field(write, "dataset"),
      field(write, "path") ?? short(field(write, "dataset")),
    ]),
  );
  for (const job of timeline.jobs)
    if (job.path) labels.set(job.dataset, job.path);
  if (!labels.has(dataset.dataset_id))
    labels.set(dataset.dataset_id, dataset.path);
  const latest = timeline.jobs
    .find((j) => j.dataset === dataset.dataset_id)
    ?.attempts.at(-1)?.attempt;
  return (
    <div className="build-detail">
      <div className="execution-toolbar">
        <Status state={timeline.state} />
        <strong>{timeline.state}</strong>
        <span title={timeline.build}>Build {short(timeline.build)}</span>
        <Button onClick={() => setRefresh((n) => n + 1)}>Refresh</Button>
        {active && (
          <Button
            onClick={() => {
              void cancel();
            }}
            disabled={canceling}
          >
            {canceling ? "Requesting…" : "Cancel build"}
          </Button>
        )}
      </div>
      {cancelMessage && <p role="status">{cancelMessage}</p>}
      <div className="metrics-summary">
        <Metric label="Queued" value={timestamp(timeline.queued_us)} />
        <Metric label="Started" value={timestamp(timeline.started_us)} />
        <Metric label="Finished" value={timestamp(timeline.finished_us)} />
        <Metric
          label="Build elapsed"
          value={seconds(
            timeline.wall_duration_us === null
              ? null
              : (BigInt(timeline.wall_duration_us) * 1000n).toString(),
          )}
        />
        <Metric
          label="Initial queue wait"
          value={seconds(
            timeline.initial_queue_wait_us === null
              ? null
              : (BigInt(timeline.initial_queue_wait_us) * 1000n).toString(),
          )}
        />
      </div>
      <div className="source-provenance">
        <span title={timeline.plan}>Plan {short(timeline.plan)}</span>
        <span title={timeline.source}>Snapshot {short(timeline.source)}</span>
        <span>
          Requested by {field(report, "requested_by") ?? "Unavailable"}
        </span>
        <span>ETA unavailable</span>
        {timeline.job_counts.map((count) => (
          <span key={count.state}>
            {count.count} {count.state.toLowerCase()}
          </span>
        ))}
      </div>
      <details>
        <summary>Targets and trigger</summary>
        <Evidence>{property(property(plan, "context"), "targets")}</Evidence>
        <Evidence>{property(report, "trigger")}</Evidence>
      </details>
      <div className="execution-toolbar">
        {timeline.critical_path ? (
          <Button
            aria-pressed={critical}
            onClick={() => setCritical(!critical)}
          >
            Critical path · {seconds(timeline.critical_path.duration_ns)}
          </Button>
        ) : (
          <span>Critical path unavailable while evidence is incomplete.</span>
        )}
        <span>
          Phase positions use recorded UTC intervals; duration labels use
          measured nanoseconds.
        </span>
      </div>
      <Gantt
        timeline={timeline}
        labels={labels}
        critical={critical}
        onAttempt={setAttempt}
      />
      {(attempt ?? latest) ? (
        <AttemptDetail
          key={attempt ?? latest}
          scope={result.scope}
          attempt={attempt ?? latest ?? ""}
        />
      ) : (
        <Empty>
          {job.state === "CACHED"
            ? `Reused version ${short(job.reused_version)}; no execution attempt. Original attempt ${short(job.original_attempt)}.`
            : "No attempt has started for this job."}
        </Empty>
      )}
    </div>
  );
}
function Gantt({
  timeline,
  labels,
  critical,
  onAttempt,
}: {
  timeline: ApiExecutionTimelineV1;
  labels: ReadonlyMap<string | undefined, string>;
  critical: boolean;
  onAttempt: (id: string) => void;
}) {
  const phases = timeline.jobs.flatMap((job) =>
    job.attempts.flatMap((attempt) => attempt.phases),
  );
  const start = BigInt(timeline.queued_us);
  const end = phases.reduce(
    (end, phase) => {
      const n = BigInt(phase.finished_us ?? phase.started_us);
      return n > end ? n : end;
    },
    BigInt(timeline.finished_us ?? timeline.started_us ?? timeline.queued_us),
  );
  const span = end > start ? end - start : 1n;
  const names = [...new Set(phases.map((phase) => phase.phase))];
  return (
    <div className="gantt">
      <div className="phase-legend">
        {names.map((name, index) => (
          <span key={name}>
            <i className={`phase-color phase-${index % 6}`} />
            {name}
          </span>
        ))}
      </div>
      <div className="gantt-axis">
        <span>{timestamp(start.toString())}</span>
        <span>{seconds((span * 1000n).toString())} recorded span</span>
      </div>
      {timeline.jobs.map((job) => (
        <div
          className={`gantt-row ${critical && timeline.critical_path?.jobs.includes(job.job) ? "critical-job" : ""}`}
          key={job.job}
        >
          <div className="gantt-label">
            <strong title={job.dataset}>
              {labels.get(job.dataset) ?? short(job.dataset)}
            </strong>
            <small>
              {job.state} ·{" "}
              {job.state === "CACHED" ? "Reuse" : seconds(job.duration_ns)}
            </small>
          </div>
          <div className="gantt-attempts">
            {!job.attempts.length ? (
              <span className="gantt-no-attempt">
                {job.state === "CACHED"
                  ? "Cached reuse · no phases"
                  : "No recorded phases"}
              </span>
            ) : (
              job.attempts.map((attempt) => (
                <div className="gantt-attempt" key={attempt.attempt}>
                  <Button onClick={() => onAttempt(attempt.attempt)}>
                    #{attempt.number} · {attempt.state}
                  </Button>
                  <div className="gantt-track">
                    {attempt.phases.map((phase, i) => (
                      <button
                        key={i}
                        type="button"
                        className={`phase-bar phase-${names.indexOf(phase.phase) % 6} ${phase.finished_us === null ? "phase-open" : ""}`}
                        style={{
                          left: `${position(BigInt(phase.started_us), start, span)}%`,
                          width:
                            phase.finished_us === null
                              ? "4px"
                              : `${Math.max(0.4, position(BigInt(phase.finished_us), BigInt(phase.started_us), span))}%`,
                        }}
                        onClick={() => onAttempt(attempt.attempt)}
                        aria-label={`${labels.get(job.dataset) ?? job.dataset} attempt ${attempt.number}: ${phase.phase}, ${phase.finished_us === null ? "running; end unavailable" : seconds(phase.duration_ns)}`}
                        title={`${phase.phase} · ${timestamp(phase.started_us)} → ${timestamp(phase.finished_us)} · ${seconds(phase.duration_ns)} (${phase.duration_ns ?? "unknown"} ns)`}
                      />
                    ))}
                  </div>
                  {attempt.phases.some((p) => p.finished_us === null) && (
                    <small>Running · end unavailable</small>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
      ))}
    </div>
  );
}
function AttemptDetail({ scope, attempt }: { scope: Scope; attempt: string }) {
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
function Logs({ scope, attempt }: { scope: Scope; attempt: string }) {
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
