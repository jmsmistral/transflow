import { useEffect, useState } from "react";
import type {
  ApiExecutionMetricsV1,
  ApiHistoryJobV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { Icon } from "./Icons";
import { field, versionMetadata } from "./inspectors";
import {
  historyDate,
  historyDateRange,
  percent,
  position,
  seconds,
  short,
  sortedHistory,
  timestamp,
} from "./execution-model";
import {
  type Ready,
  type Version,
  type HistoryResult,
  branchSelection,
  message,
  Empty,
  Fallback,
  Metric,
  Status,
} from "./execution-ui";
import { JobHistoryDetail } from "./JobHistoryDetail";
export function HistoryInspector({
  workspace,
  state,
  onVersion,
}: {
  workspace: Workspace;
  state: Ready;
  onVersion: (version: Version | null) => void;
}) {
  const dataset = state.value.dataset;
  const [history, setHistory] = useState<HistoryResult>();
  const [measured, setMeasured] = useState<{
    key: string;
    value: ApiExecutionMetricsV1;
  }>();
  const [selected, setSelected] = useState<ApiHistoryJobV1>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [from, setFrom] = useState(() =>
    new Date(Date.now() - 30 * 86400000).toISOString().slice(0, 10),
  );
  const [to, setTo] = useState(() => new Date().toISOString().slice(0, 10));
  const [range, setRange] = useState({ from, to });
  const rangeKey = JSON.stringify(range);
  const metrics = measured?.key === rangeKey ? measured.value : undefined;
  const validRange = historyDateRange(from, to);
  useEffect(() => {
    if (!dataset || dataset.origin === "external") return;
    const bounds = historyDateRange(range.from, range.to);
    if (!bounds) return;
    const abort = new AbortController();
    void (async () => {
      const scope = await workspace.scope(branchSelection(state), abort.signal);
      const [page, measured] = await Promise.all([
        scope.read(
          "ApiDatasetHistoryV1",
          `/api/v1/datasets/${dataset.dataset_id}/history`,
          { limit: "50" },
        ),
        scope.read("ApiExecutionMetricsV1", "/api/v1/metrics", {
          dataset: dataset.dataset_id,
          ...bounds,
          window: "10",
          materialized_any: "false",
        }),
      ]);
      return { page, measured };
    })().then(
      ({ page, measured }) => {
        if (!abort.signal.aborted) {
          setHistory({ entries: page.entries, next: page.next_cursor });
          setMeasured({ key: rangeKey, value: measured });
          setError("");
        }
      },
      (error) => {
        if (!abort.signal.aborted) setError(message(error));
      },
    );
    return () => abort.abort();
  }, [workspace, state, dataset, range, rangeKey]);
  async function more() {
    if (!dataset || busy) return;
    setBusy(true);
    const abort = new AbortController();
    try {
      const scope = await workspace.scope(branchSelection(state), abort.signal);
      if (history?.next && history.entries.length < 1000) {
        const page = await scope.read(
          "ApiDatasetHistoryV1",
          `/api/v1/datasets/${dataset.dataset_id}/history`,
          { cursor: history.next, limit: "50" },
        );
        setHistory({
          entries: [...history.entries, ...page.entries],
          next: page.next_cursor,
        });
      }
    } catch (error) {
      setError(message(error));
    } finally {
      setBusy(false);
    }
  }
  const jobs = sortedHistory(history?.entries ?? []);
  const meta = dataset
    ? versionMetadata(
        dataset,
        state.value.versions,
        state.selection.version,
        state.value.inspection,
      )
    : null;
  return (
    <div className="execution-inspector history-inspector">
      {!state.selection.version && (
        <Fallback
          branch={meta?.originBranch ?? field(dataset?.head, "resolved_branch")}
          requested={state.selection.branch}
        />
      )}
      <div className="execution-toolbar">
        <strong>{dataset?.path}</strong>
        <span className="history-branch" title="History branch">
          <Icon name="branch" /> {state.selection.branch}
        </span>
        <div className="execution-switches">
          <Button disabled={!selected} onClick={() => setSelected(undefined)}>
            Show summary
          </Button>
        </div>
        {state.selection.version && (
          <Button onClick={() => onVersion(null)}>Return to branch tip</Button>
        )}
      </div>
      {state.selection.version && (
        <div className="source-provenance">
          Preview and Code are pinned to version{" "}
          {short(state.selection.version)}. Jobs and metrics below query branch{" "}
          {state.selection.branch}.
        </div>
      )}
      {error && (
        <p className="execution-error" role="alert">
          {error}
        </p>
      )}
      {dataset?.origin === "external" ? (
        <Empty>
          Execution history is retained by the provider workspace and is
          unavailable here.
        </Empty>
      ) : (
        <div className="history-layout">
          <div className="history-list" aria-label="Dataset jobs">
            {jobs.map((job) => (
              <button
                className="history-entry"
                key={job.id}
                aria-pressed={selected?.id === job.id}
                onClick={() => setSelected(job)}
              >
                <Status state={job.state} />
                <span className="history-entry-copy">
                  <strong title={timestamp(job.created_us)}>
                    {timestamp(job.created_us)}
                  </strong>
                  <small title={job.id}>
                    {job.state} ·{" "}
                    {job.state === "CACHED" && "Reuse (no execution) · "}
                    {job.attempt_count} attempts
                  </small>
                  <small className="history-build" title={`Build ${job.build}`}>
                    Part of <Status state={job.build_state} />
                    <span>
                      {job.build_state} build · {job.build_job_count}{" "}
                      {job.build_job_count === "1" ? "job" : "jobs"}
                    </span>
                  </small>
                </span>
                {job.state !== "CACHED" && (
                  <small className="history-job-duration" title="Job duration">
                    ({seconds(job.duration_ns)})
                  </small>
                )}
              </button>
            ))}
            {history && !jobs.length && (
              <Empty>No retained jobs on this branch.</Empty>
            )}
            {history?.next && (
              <Button
                onClick={() => {
                  void more();
                }}
                disabled={busy || jobs.length >= 1000}
              >
                {busy ? "Loading…" : "Load more"}
              </Button>
            )}
          </div>
          <div className="history-detail">
            {selected && dataset ? (
              <JobHistoryDetail
                key={selected.id}
                workspace={workspace}
                state={state}
                job={selected}
                dataset={dataset}
              />
            ) : (
              <>
                <form
                  className="metrics-range"
                  onSubmit={(event) => {
                    event.preventDefault();
                    if (validRange) setRange({ from, to });
                  }}
                >
                  <label>
                    From
                    <input
                      type="text"
                      placeholder="YYYY-MM-DD"
                      pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}"
                      title="YYYY-MM-DD"
                      value={from}
                      onChange={(event) => setFrom(event.target.value)}
                      required
                    />
                  </label>
                  <label>
                    To
                    <input
                      type="text"
                      placeholder="YYYY-MM-DD"
                      pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}"
                      title="YYYY-MM-DD"
                      value={to}
                      onChange={(event) => setTo(event.target.value)}
                      required
                    />
                  </label>
                  <Button type="submit" disabled={!validRange}>
                    Apply
                  </Button>
                </form>
                {metrics ? (
                  <>
                    <div className="metrics-summary">
                      <Metric label="Builds" value={metrics.builds} />
                      <Metric
                        label="Failure rate"
                        value={percent(metrics.failure_rate)}
                        title={
                          metrics.failure_rate
                            ? `${metrics.failure_rate.numerator}/${metrics.failure_rate.denominator} succeeded + failed builds`
                            : "No succeeded or failed builds"
                        }
                      />
                      <Metric
                        label="Median duration"
                        value={seconds(metrics.median_ns)}
                      />
                      <Metric
                        label={`Mean · latest ${metrics.trailing_window} successes`}
                        value={seconds(metrics.trailing_mean_ns)}
                      />
                      <Metric
                        label="Measured / missing"
                        value={`${metrics.duration_samples} / ${metrics.missing_duration_samples}`}
                      />
                    </div>
                    <DurationChart jobs={jobs} metrics={metrics} />
                    <div className="source-provenance">
                      {metrics.manual_requests} manual requests ·{" "}
                      {metrics.scheduled_builds} scheduled builds ·{" "}
                      {metrics.jobs_executed} executed jobs · {metrics.attempts}{" "}
                      attempts
                    </div>
                  </>
                ) : (
                  !error && <Empty>Loading execution history…</Empty>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
function DurationChart({
  jobs,
  metrics,
}: {
  jobs: readonly ApiHistoryJobV1[];
  metrics: ApiExecutionMetricsV1;
}) {
  const samples = jobs
    .filter(
      (job) =>
        BigInt(job.created_us) >= BigInt(metrics.from_us) &&
        BigInt(job.created_us) < BigInt(metrics.to_us),
    )
    .flatMap((job) =>
      job.attempts
        .filter(
          (attempt) =>
            attempt.duration_ns !== null &&
            (attempt.state === "SUCCEEDED" || attempt.state === "FAILED"),
        )
        .map((attempt) => ({ ...attempt, accepted: job.created_us })),
    );
  const mean = metrics.trailing_mean_ns
    ? BigInt(metrics.trailing_mean_ns.numerator) /
      BigInt(metrics.trailing_mean_ns.denominator)
    : null;
  const max =
    samples.reduce(
      (n, sample) =>
        BigInt(sample.duration_ns ?? "0") > n
          ? BigInt(sample.duration_ns ?? "0")
          : n,
      mean ?? 1n,
    ) || 1n;
  return (
    <div className="duration-chart">
      <div className="chart-caption" title="Both dates are included">
        Attempt duration (seconds) · {samples.length} measured successes /
        failures in {jobs.length} loaded jobs · {historyDate(metrics.from_us)}{" "}
        to {historyDate(metrics.to_us, true)}
      </div>
      {samples.length ? (
        <svg
          viewBox="0 0 700 180"
          role="img"
          aria-label="Measured attempt durations by build acceptance time"
        >
          <title>
            Measured attempt durations, successful and failed. Cached reuse has
            no point.
          </title>
          <line x1="58" y1="12" x2="58" y2="145" stroke="currentColor" />
          <line x1="58" y1="145" x2="685" y2="145" stroke="currentColor" />
          <text x="2" y="18">
            {seconds(max.toString())}
          </text>
          <text x="20" y="145">
            0 s
          </text>
          {mean !== null && (
            <>
              <line
                className="duration-mean"
                x1="58"
                x2="685"
                y1={145 - position(mean, 0n, max) * 1.25}
                y2={145 - position(mean, 0n, max) * 1.25}
              />
              <text x="70" y="174">
                Mean of latest {metrics.trailing_samples} successful
                materializations in this range:{" "}
                {seconds(metrics.trailing_mean_ns)}
              </text>
            </>
          )}
          {samples.map((sample) => (
            <circle
              className={`duration-point status-${sample.state.toLowerCase()}`}
              key={sample.attempt}
              cx={
                58 +
                position(
                  BigInt(sample.accepted),
                  BigInt(metrics.from_us),
                  BigInt(metrics.to_us) - BigInt(metrics.from_us),
                ) *
                  6.25
              }
              cy={
                145 -
                position(BigInt(sample.duration_ns ?? "0"), 0n, max) * 1.25
              }
              r="4"
            >
              <title>
                {sample.state} · {timestamp(sample.accepted)} ·{" "}
                {seconds(sample.duration_ns)} ({sample.duration_ns} ns) ·
                attempt {sample.attempt}
              </title>
            </circle>
          ))}
        </svg>
      ) : (
        <Empty>
          No measured attempt durations in the loaded jobs for this range.
        </Empty>
      )}
    </div>
  );
}
