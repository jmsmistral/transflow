import { useEffect, useId, useRef, useState } from "react";
import type {
  ApiDatasetV1,
  ApiHistoryJobV1,
  ApiExecutionTimelineV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { Icon } from "./Icons";
import { field } from "./inspectors";
import { position, seconds, short, timestamp } from "./execution-model";
import {
  type Ready,
  Empty,
  Status,
  Evidence,
  property,
  items,
} from "./execution-ui";
import {
  type BuildEvidence,
  orderedJobs,
  terminalBuild,
  useBuildClock,
  useBuildEvidence,
} from "./build-evidence";
import { AttemptDetail } from "./BuildInspectors";

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
  const evidence = useBuildEvidence(workspace, state, job);
  const [open, setOpen] = useState(false);
  return (
    <>
      <div className="execution-toolbar">
        <Status state={evidence.result?.timeline.state ?? job.build_state} />
        Build {short(job.build)}
        <Button onClick={() => setOpen(true)}>
          <Icon name="build" /> View build report
        </Button>
      </div>
      <Empty>
        Open the build report to inspect all jobs and live progress.
      </Empty>
      {open && (
        <BuildModal
          {...evidence}
          dataset={dataset}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}
export function BuildModal({
  result,
  error,
  refresh,
  dataset,
  onClose,
}: {
  result: BuildEvidence | undefined;
  error: string;
  refresh: () => void;
  dataset: ApiDatasetV1;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const title = useId();
  const targets = items(
    property(property(property(result?.report, "plan"), "context"), "targets"),
  ).filter((value): value is string => typeof value === "string");
  useEffect(() => {
    const dialog = ref.current;
    const previous = document.activeElement;
    dialog?.showModal();
    return () => {
      dialog?.close();
      if (previous instanceof HTMLElement && previous.isConnected)
        previous.focus();
    };
  }, []);
  return (
    <dialog
      ref={ref}
      className="build-modal"
      aria-labelledby={title}
      onCancel={onClose}
    >
      <header className="build-modal-header">
        <Icon name="build" />
        <h2 id={title}>Build report</h2>
        <span>
          {targets.length
            ? `Build of: ${targets.join(", ")}`
            : `Opened from: ${dataset.path}`}
        </span>
        <Button aria-label="Close build report" onClick={onClose}>
          <Icon name="close" />
        </Button>
      </header>
      {error && (
        <p role="alert" className="execution-error">
          Build refresh failed: {error} <Button onClick={refresh}>Retry</Button>
        </p>
      )}
      {result ? (
        <BuildReport evidence={result} refresh={refresh} />
      ) : (
        <Empty>Loading plan-bound build evidence…</Empty>
      )}
    </dialog>
  );
}
function BuildReport({
  evidence,
  refresh,
}: {
  evidence: BuildEvidence;
  refresh: () => void;
}) {
  const { timeline, report, scope } = evidence;
  const active = !terminalBuild(timeline.state);
  const now = useBuildClock(active);
  const [mode, setMode] = useState<"status" | "phases" | "critical">("status");
  const [gantt, setGantt] = useState(true);
  const [filter, setFilter] = useState("");
  const [status, setStatus] = useState("");
  const [attempt, setAttempt] = useState<string>();
  const [canceling, setCanceling] = useState(false);
  const [cancelMessage, setCancelMessage] = useState("");
  const [cancelKey] = useState(() => crypto.randomUUID());
  const labels = new Map(
    items(property(property(report, "plan"), "writes")).map((w) => [
      field(w, "dataset"),
      field(w, "path"),
    ]),
  );
  const name = (j: ApiExecutionTimelineV1["jobs"][number]) =>
    j.path ?? labels.get(j.dataset) ?? short(j.dataset);
  const jobs = orderedJobs(timeline.jobs).filter(
    (j) =>
      name(j).toLowerCase().includes(filter.toLowerCase()) &&
      (!status || j.state === status),
  );
  const elapsed =
    timeline.wall_duration_us ??
    (active
      ? (now > BigInt(timeline.queued_us)
          ? now - BigInt(timeline.queued_us)
          : 0n
        ).toString()
      : null);
  const trigger = property(report, "trigger");
  const scheduled = field(trigger, "kind") === "schedule";
  async function cancel() {
    setCanceling(true);
    try {
      const response = await scope.cancel(timeline.build, cancelKey);
      setCancelMessage(
        response.disposition === "TooLate"
          ? "This build has already finished."
          : "Cancellation requested.",
      );
      refresh();
    } catch (error) {
      setCancelMessage(
        error instanceof Error ? error.message : "Cancellation failed.",
      );
    } finally {
      setCanceling(false);
    }
  }
  return (
    <div className="build-report-layout">
      <aside className="build-info">
        <h3>Build info</h3>
        <dl className="job-facts">
          <dt>Status</dt>
          <dd>
            <Status state={timeline.state} /> {timeline.state}
          </dd>
          <dt>{active ? "Elapsed (live)" : "Duration"}</dt>
          <dd>
            {seconds(
              elapsed === null ? null : (BigInt(elapsed) * 1000n).toString(),
            )}
          </dd>
          <dt>Estimated</dt>
          <dd>Unavailable</dd>
          <dt>Started</dt>
          <dd>{timestamp(timeline.started_us)}</dd>
          <dt>Ended</dt>
          <dd>{timestamp(timeline.finished_us)}</dd>
          <dt>Started by</dt>
          <dd>{field(report, "requested_by") ?? "Unavailable"}</dd>
          <dt>Progress</dt>
          <dd>
            {timeline.job_counts.find((c) => c.state === "SUCCEEDED")?.count ??
              "0"}{" "}
            of {timeline.jobs.length} jobs succeeded
          </dd>
          <dt>Build ID</dt>
          <dd className="selectable-id">{timeline.build}</dd>
          <dt>Queued</dt>
          <dd>{timestamp(timeline.queued_us)}</dd>
          <dt>Queue wait</dt>
          <dd>
            {seconds(
              timeline.initial_queue_wait_us === null
                ? null
                : (BigInt(timeline.initial_queue_wait_us) * 1000n).toString(),
            )}
          </dd>
        </dl>
        {scheduled && (
          <section className="build-schedule">
            <h3>
              <Icon name="calendar" /> Build schedule
            </h3>
            <strong>
              {field(trigger, "name") ??
                field(trigger, "schedule_id") ??
                "Retained schedule trigger"}
            </strong>
            <h4>When to build</h4>
            <Evidence>{trigger}</Evidence>
            <p>Schedule conditions and recent builds are unavailable.</p>
          </section>
        )}
      </aside>
      <main className="build-progress">
        <div className="execution-toolbar">
          <h3>Build progress</h3>
          <span className="build-live" role="status">
            {active ? "Live · updates automatically" : "Completed"}
          </span>
          <Button onClick={refresh} aria-label="Refresh" title="Refresh">
            <Icon name="refresh" />
          </Button>
          {active && (
            <Button
              disabled={canceling}
              onClick={() => {
                void cancel();
              }}
            >
              {canceling ? "Requesting…" : "Cancel build"}
            </Button>
          )}
        </div>
        {cancelMessage && <p role="status">{cancelMessage}</p>}
        <div className="execution-toolbar build-filters">
          <label>
            <input
              type="checkbox"
              checked={gantt}
              onChange={(e) => setGantt(e.target.checked)}
            />{" "}
            Gantt chart
          </label>
          <label>
            Job status{" "}
            <select value={status} onChange={(e) => setStatus(e.target.value)}>
              <option value="">All statuses</option>
              {timeline.job_counts.map((c) => (
                <option key={c.state}>{c.state}</option>
              ))}
            </select>
          </label>
          <input
            aria-label="Dataset path"
            placeholder="Dataset path…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <div className="execution-switches">
            {(
              [
                ["status", "Job status"],
                ["phases", "Progress details"],
                ["critical", "Critical path"],
              ] as const
            ).map(([value, label]) => (
              <Button
                key={value}
                aria-pressed={mode === value}
                disabled={
                  value === "critical" && (active || !timeline.critical_path)
                }
                title={
                  value === "critical" && (active || !timeline.critical_path)
                    ? "Available for completed builds with complete timing evidence"
                    : undefined
                }
                onClick={() => setMode(value)}
              >
                {label}
              </Button>
            ))}
          </div>
        </div>
        {gantt && (
          <BuildGantt
            timeline={timeline}
            displayJobs={jobs}
            mode={active && mode === "critical" ? "status" : mode}
            now={now}
            name={name}
            onAttempt={setAttempt}
          />
        )}
        <table className="build-jobs">
          <thead>
            <tr>
              <th>Dataset</th>
              <th>Start time</th>
              <th>Duration</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {jobs.map((j) => (
              <tr key={j.job}>
                <td>
                  <strong>{name(j)}</strong>
                  <small>
                    {j.attempts.length}{" "}
                    {j.attempts.length === 1 ? "attempt" : "attempts"}
                  </small>
                </td>
                <td>{timestamp(j.attempts[0]?.started_us ?? null)}</td>
                <td>
                  {j.state === "CACHED"
                    ? "Cached reuse"
                    : seconds(j.duration_ns)}
                </td>
                <td>
                  <Status state={j.state} /> {j.state}
                  {j.attempts.at(-1) && (
                    <Button
                      onClick={() => setAttempt(j.attempts.at(-1)?.attempt)}
                    >
                      View attempt
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {!jobs.length && <Empty>No jobs match these filters.</Empty>}
        {attempt && (
          <section>
            <div className="execution-toolbar attempt-evidence-toolbar">
              <h3>Attempt evidence</h3>
              <Button onClick={() => setAttempt(undefined)}>Close</Button>
            </div>
            <AttemptDetail key={attempt} scope={scope} attempt={attempt} />
          </section>
        )}
      </main>
    </div>
  );
}
export function BuildGantt({
  timeline,
  displayJobs = timeline.jobs,
  mode,
  now,
  name,
  onAttempt,
}: {
  timeline: ApiExecutionTimelineV1;
  displayJobs?: ApiExecutionTimelineV1["jobs"];
  mode: "status" | "phases" | "critical";
  now: bigint;
  name: (job: ApiExecutionTimelineV1["jobs"][number]) => string;
  onAttempt: (id: string) => void;
}) {
  const active = !terminalBuild(timeline.state);
  const phases = timeline.jobs.flatMap((j) =>
    j.attempts.flatMap((a) => a.phases),
  );
  // Exclude initial build queue wait, but keep one axis across all job filters.
  const starts = timeline.jobs.flatMap((j) =>
    j.attempts.flatMap((a) => [
      a.started_us,
      ...a.phases.map((p) => p.started_us),
    ]),
  );
  const start =
    starts.reduce<bigint | undefined>((first, value) => {
      const time = BigInt(value);
      return first === undefined || time < first ? time : first;
    }, undefined) ?? BigInt(timeline.queued_us);
  const end = phases.reduce(
    (end, p) => {
      const t = BigInt(p.finished_us ?? p.started_us);
      return t > end ? t : end;
    },
    active
      ? now > start
        ? now
        : start
      : BigInt(timeline.finished_us ?? timeline.queued_us),
  );
  const span = end > start ? end - start : 1n;
  const names = [...new Set(phases.map((p) => p.phase))];
  const path = timeline.critical_path?.jobs ?? [];
  const longest = orderedJobs(timeline.jobs)
    .filter((j) => path.includes(j.job))
    .reduce<string | undefined>(
      (id, j) =>
        BigInt(j.duration_ns ?? "0") >
        BigInt(timeline.jobs.find((x) => x.job === id)?.duration_ns ?? "-1")
          ? j.job
          : id,
      undefined,
    );
  const legend =
    mode === "phases"
      ? names
      : mode === "critical"
        ? ["Most critical", "On critical path", "Non-critical"]
        : [...new Set(timeline.jobs.map((j) => j.state))];
  return (
    <section className="build-gantt" aria-label="Build Gantt chart">
      <div className="phase-legend">
        {legend.map((label, i) => (
          <span key={label}>
            <i
              className={`phase-color ${mode === "phases" ? `phase-${i % 6}` : mode === "critical" ? `critical-${i}` : `job-bar-${label.toLowerCase()}`}`}
            />
            {label}
          </span>
        ))}
      </div>
      {orderedJobs(displayJobs).map((j) => (
        <div className="gantt-row" key={j.job}>
          <div className="gantt-label">
            <strong>{name(j)}</strong>
          </div>
          <div>
            {!j.attempts.length ? (
              <span className="gantt-no-attempt">
                {j.state === "CACHED"
                  ? "Cached reuse · no execution"
                  : "Not started"}
              </span>
            ) : (
              j.attempts.map((a) => {
                const bars =
                  mode === "phases"
                    ? a.phases
                    : [
                        {
                          phase: j.state,
                          started_us: a.started_us,
                          finished_us: a.finished_us,
                          duration_ns: a.duration_ns,
                        },
                      ];
                return (
                  <div className="gantt-track" key={a.attempt}>
                    {bars.map((p, i) => (
                      <button
                        key={i}
                        className={`phase-bar ${mode === "phases" ? `phase-${names.indexOf(p.phase) % 6}` : mode === "critical" ? `critical-${j.job === longest ? 0 : path.includes(j.job) ? 1 : 2}` : `job-bar-${j.state.toLowerCase()}`} ${p.finished_us === null ? "phase-open" : ""}`}
                        style={{
                          left: `${position(BigInt(p.started_us), start, span)}%`,
                          width: `${Math.max(0.35, position(p.finished_us === null ? (active ? end : BigInt(p.started_us)) : BigInt(p.finished_us), BigInt(p.started_us), span))}%`,
                        }}
                        onClick={() => onAttempt(a.attempt)}
                        aria-label={`${name(j)} attempt ${a.number}: ${p.phase}, ${p.finished_us === null ? (active ? "in progress; elapsed only" : "end unavailable") : seconds(p.duration_ns)}`}
                        title={`${p.phase} · ${timestamp(p.started_us)} → ${p.finished_us === null ? (active ? "In progress (live elapsed, not a completion estimate)" : "End unavailable") : timestamp(p.finished_us)}`}
                      >
                        <span aria-hidden="true">
                          {mode === "critical"
                            ? j.job === longest
                              ? "!"
                              : path.includes(j.job)
                                ? "◷"
                                : "·"
                            : j.state === "SUCCEEDED"
                              ? "✓"
                              : j.state === "FAILED"
                                ? "×"
                                : "·"}
                        </span>
                      </button>
                    ))}
                  </div>
                );
              })
            )}
          </div>
        </div>
      ))}
      <div className="gantt-time-axis">
        {[0, 1, 2, 3, 4].map((i) => (
          <span key={i}>
            {timestamp((start + (span * BigInt(i)) / 4n).toString())}
          </span>
        ))}
      </div>
    </section>
  );
}
