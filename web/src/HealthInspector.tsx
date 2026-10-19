import { useEffect, useId, useState } from "react";
import type {
  ApiLineageNodeV1,
  ApiOverlayFactsV1,
  ApiHistoryJobV1,
  ApiDatasetHistoryV1,
  ExecutionJsonV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { field, versionMetadata } from "./inspectors";
import { timestamp } from "./execution-model";
import { checkStatus, buildStatus, type HealthPoint } from "./health-model";
import {
  Empty,
  Fallback,
  branchSelection,
  items,
  message,
  property,
  type Ready,
} from "./execution-ui";

export function healthLabel(value: ApiOverlayFactsV1["health"] | undefined) {
  return value === "NoChecks" ? "No output checks" : (value ?? "Unavailable");
}
function Checks({
  values,
  empty,
}: {
  values: readonly ExecutionJsonV1[] | undefined;
  empty: string;
}) {
  const [limit, setLimit] = useState(20);
  return (
    <div className="health-selected-checks">
      {!values ? (
        <Empty>Check evidence is unavailable.</Empty>
      ) : !values.length ? (
        <p className="muted">{empty}</p>
      ) : (
        <div className="health-checks">
          {values.slice(0, limit).map((check, index) => {
            const metrics = property(check, "metrics");
            const declaration = property(
              property(check, "policy"),
              "declaration",
            );
            const name =
              field(metrics, "name") ??
              field(declaration, "name") ??
              field(check, "name") ??
              `Certificate ${index + 1}`;
            const severity =
              field(property(check, "policy"), "severity") ??
              field(metrics, "severity");
            const alias = field(check, "alias");
            const status = checkStatus([check]);
            return (
              <article
                className="health-check"
                key={
                  field(check, "id") ??
                  field(check, "fingerprint") ??
                  String(index)
                }
              >
                <div className="health-check-heading">
                  <span
                    className={`health-dot health-${status.tone}`}
                    aria-hidden="true"
                  />
                  <strong>{name}</strong>
                  <span className="health-outcome">{status.label}</span>
                </div>
                {(severity || alias) && (
                  <small>
                    {[
                      severity
                        ? `On violation: ${severity === "FAIL" ? "fail build" : severity === "WARN" ? "warn" : severity}`
                        : null,
                      alias ? `Input ${alias}` : null,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </small>
                )}
                {field(metrics, "failed_rows") !== undefined && (
                  <p>Violating rows: {field(metrics, "failed_rows")}</p>
                )}
                {field(metrics, "error") && (
                  <p className="execution-error">{field(metrics, "error")}</p>
                )}
                <details>
                  <summary>Check details</summary>
                  <pre className="execution-json">
                    {JSON.stringify(check, null, 2)}
                  </pre>
                </details>
              </article>
            );
          })}
          {limit < values.length && (
            <Button
              onClick={() => setLimit((n) => Math.min(n + 20, values.length))}
            >
              More checks ({values.length - limit} remaining)
            </Button>
          )}
        </div>
      )}
    </div>
  );
}

function HistoryStrip({
  title,
  points,
  loading,
  error,
  interactive = true,
}: {
  title: string;
  points: readonly HealthPoint[];
  loading?: boolean;
  error?: string | undefined;
  interactive?: boolean;
}) {
  const latest = points[0];
  const detailsId = useId();
  const [selected, setSelected] = useState<number>();
  const [expanded, setExpanded] = useState(false);
  const point = selected === undefined ? undefined : points[selected];
  return (
    <section className="health-section" aria-label={title}>
      <div className="health-history-heading">
        <h3>{title}</h3>
        <span className={`health-pill health-${latest?.tone ?? "neutral"}`}>
          {loading ? "Loading…" : (latest?.label ?? "No history")}
        </span>
      </div>
      <ol className="health-history" aria-label={`${title}, newest first`}>
        {Array.from({ length: 10 }, (_, index) => {
          const point = points[index];
          const label = point
            ? `${point.label}${point.at ? ` · ${timestamp(point.at, "minute")}` : ""}`
            : "No recorded history";
          return (
            <li key={index} aria-label={`${index + 1}: ${label}`}>
              {interactive ? (
                <button
                  type="button"
                  className={`health-dot health-${point?.tone ?? "neutral"}`}
                  title={label}
                  aria-label={`${index + 1}: ${label}`}
                  aria-expanded={expanded && selected === index}
                  aria-controls={detailsId}
                  disabled={!point}
                  onClick={() => {
                    setSelected(index);
                    setExpanded(selected !== index || !expanded);
                  }}
                />
              ) : (
                <span
                  className={`health-dot health-${point?.tone ?? "neutral"}`}
                  title={label}
                  role="img"
                  aria-label={`${index + 1}: ${label}`}
                />
              )}
            </li>
          );
        })}
      </ol>
      {error && (
        <p className="execution-error" role="alert">
          {error}
        </p>
      )}
      {interactive && (
        <div
          id={detailsId}
          className="health-history-details"
          data-expanded={expanded}
          aria-hidden={!expanded}
          inert={!expanded}
        >
          <div>
            {point && (
              <>
                <p className="health-selected-at">
                  {point.label}
                  {point.at ? ` · ${timestamp(point.at, "minute")}` : ""}
                </p>
                <Checks
                  key={selected}
                  values={point.checks}
                  empty={point.note ?? "No checks were recorded."}
                />
                {point.note && point.checks === undefined && (
                  <p className="muted">{point.note}</p>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

export function HealthInspector({
  workspace,
  state,
}: {
  workspace: Workspace;
  state: Ready;
  node: ApiLineageNodeV1 | undefined;
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
  const key = state.value.context.fingerprint;
  const revision = state.value.context.runtime_revision;
  const [result, setResult] = useState<{
    key: string;
    revision: string;
    builds: HealthPoint[];
    outputs: HealthPoint[];
    inputs: HealthPoint[];
    error?: string | undefined;
    limited?: boolean;
  }>();
  useEffect(() => {
    if (!dataset || dataset.origin === "external") return;
    const abort = new AbortController();
    void (async () => {
      const scope = await workspace.scope(branchSelection(state), abort.signal);
      const jobs: ApiHistoryJobV1[] = [];
      let cursor: string | null = null;
      do {
        const page: ApiDatasetHistoryV1 = await scope.read(
          "ApiDatasetHistoryV1",
          `/api/v1/datasets/${dataset.dataset_id}/history`,
          { limit: "50", order: "newest", ...(cursor ? { cursor } : {}) },
        );
        jobs.push(...page.entries);
        cursor = page.next_cursor;
      } while (
        cursor &&
        jobs.length < 1000 &&
        (jobs.flatMap((j) => j.attempts).length < 10 ||
          jobs.filter((j) => j.produced_version).length < 10)
      );
      const timings = jobs
        .flatMap((job) => job.attempts.map((t) => ({ ...t, owner: job })))
        .sort((a, b) =>
          BigInt(a.started_us) > BigInt(b.started_us)
            ? -1
            : BigInt(a.started_us) < BigInt(b.started_us)
              ? 1
              : b.attempt.localeCompare(a.attempt),
        );
      const inputs: HealthPoint[] = [];
      const errors: string[] = [];
      const attemptPoints = new Map<string, HealthPoint>();
      const buildAttempts = jobs.slice(0, 10).flatMap((job) => {
        const latest = [...timings].find(
          (timing) => timing.owner.id === job.id,
        );
        return latest ? [latest] : [];
      });
      const requested = [
        ...new Map(
          [...timings.slice(0, 10), ...buildAttempts].map((timing) => [
            timing.attempt,
            timing,
          ]),
        ).values(),
      ];
      for (const timing of requested) {
        try {
          const retained = await workspace.scope(
            { branch: state.selection.branch, plan: timing.owner.plan },
            abort.signal,
          );
          const value = await retained.read(
            "ApiAttemptV1",
            `/api/v1/attempts/${timing.attempt}`,
          );
          if (
            value.attempt !== timing.attempt ||
            value.dataset !== dataset.dataset_id ||
            value.plan !== timing.owner.plan ||
            value.build !== timing.owner.build ||
            value.source !== timing.owner.source
          )
            throw new Error("Check evidence does not match this dataset.");
          const checks = items(property(value.evidence, "checks"));
          attemptPoints.set(timing.attempt, { ...checkStatus(checks), checks });
        } catch (error) {
          if (abort.signal.aborted) return;
          errors.push(message(error));
          attemptPoints.set(timing.attempt, {
            label: "Unavailable",
            tone: "neutral",
            note: message(error),
          });
        }
      }
      for (const timing of timings.slice(0, 10)) {
        const retained = attemptPoints.get(timing.attempt);
        const checks = retained?.checks?.filter(
          (c) => field(c, "phase") === "input",
        );
        inputs.push({
          ...(checks
            ? { ...checkStatus(checks), checks }
            : (retained ?? { label: "Unavailable", tone: "neutral" })),
          at: timing.started_us,
        });
      }
      const publications = jobs.flatMap((j) =>
        j.produced_version
          ? [
              {
                version: j.produced_version,
                attempt: [...j.attempts]
                  .reverse()
                  .find((a) => a.state === "SUCCEEDED")?.attempt,
                at: j.finished_us ?? j.created_us,
              },
            ]
          : [],
      );
      if (
        meta?.version &&
        !publications.some((p) => p.version === meta.version)
      )
        publications.unshift({
          version: meta.version,
          attempt: meta.attempt ?? undefined,
          at: meta.published ?? "",
        });
      publications.sort((a, b) =>
        BigInt(a.at || "0") > BigInt(b.at || "0")
          ? -1
          : BigInt(a.at || "0") < BigInt(b.at || "0")
            ? 1
            : b.version.localeCompare(a.version),
      );
      const outputs: HealthPoint[] = [];
      for (const publication of publications.slice(0, 10)) {
        if (!publication.attempt) {
          outputs.push({
            label: dataset.kind === "imported" ? "No checks" : "Unavailable",
            tone: "neutral",
            at: publication.at,
          });
          continue;
        }
        try {
          const retained = await workspace.scope(
            {
              branch: state.selection.branch,
              dataset: dataset.dataset_id,
              origin: dataset.workspace_id,
              version: publication.version,
            },
            abort.signal,
          );
          const value = await retained.read(
            "ApiAttemptV1",
            `/api/v1/attempts/${publication.attempt}`,
          );
          if (
            value.attempt !== publication.attempt ||
            value.dataset !== dataset.dataset_id
          )
            throw new Error(
              "Published check evidence does not match this dataset.",
            );
          const checks = items(property(value.evidence, "checks")).filter(
            (c) => field(c, "phase") === "output",
          );
          outputs.push({ ...checkStatus(checks), checks, at: publication.at });
        } catch (error) {
          if (abort.signal.aborted) return;
          errors.push(message(error));
          outputs.push({
            label: "Unavailable",
            tone: "neutral",
            at: publication.at,
            note: message(error),
          });
        }
      }
      if (!abort.signal.aborted)
        setResult({
          key,
          revision,
          builds: jobs.slice(0, 10).map((j) => {
            const latest = buildAttempts.find((t) => t.owner.id === j.id);
            const retained = latest
              ? attemptPoints.get(latest.attempt)
              : undefined;
            return {
              ...(j.state === "SUCCEEDED" && retained?.tone === "warn"
                ? { label: "Warning", tone: "warn" as const }
                : buildStatus(j.state)),
              at: j.created_us,
              checks: j.state === "CACHED" ? [] : retained?.checks,
              note:
                j.state === "CACHED"
                  ? "Cached reuse did not run checks. Published check evidence is shown separately."
                  : (retained?.note ??
                    (!latest
                      ? "No execution attempt was recorded."
                      : undefined)),
            };
          }),
          outputs,
          inputs,
          limited: !!cursor && jobs.length >= 1000,
          error: errors.length ? [...new Set(errors)].join("; ") : undefined,
        });
    })().catch((error) => {
      if (!abort.signal.aborted)
        setResult({
          key,
          revision,
          builds: [],
          outputs: [],
          inputs: [],
          error: message(error),
        });
    });
    return () => abort.abort();
  }, [
    workspace,
    state,
    dataset,
    key,
    meta?.version,
    meta?.attempt,
    meta?.published,
    revision,
  ]);
  if (!dataset) return <Empty>Select a dataset to inspect its health.</Empty>;
  const current =
    result?.key === key && result.revision === revision ? result : undefined;
  const foreign = dataset.origin === "external";
  const entry = state.value.versions?.entries.find(
    (v) => v.version === meta?.version,
  );
  const provider =
    entry?.origin === "external"
      ? property(entry.metadata, "output_certificates")
      : undefined;
  const outputChecks = Array.isArray(provider) ? provider : undefined;
  const outputs = foreign
    ? outputChecks
      ? [
          {
            ...checkStatus(outputChecks),
            checks: outputChecks,
            at: meta?.published ?? undefined,
          },
        ]
      : []
    : (current?.outputs ?? []);
  return (
    <div className="health-inspector">
      {!state.selection.version && (
        <Fallback
          branch={meta?.originBranch ?? field(dataset.head, "resolved_branch")}
          requested={state.selection.branch}
        />
      )}
      <header className="health-dataset-header">
        <h2 title={dataset.path}>{dataset.path}</h2>
      </header>
      <div className="health-content">
        <HistoryStrip
          key={`${key}:${revision}:builds`}
          title="Latest builds"
          interactive={false}
          points={current?.builds ?? []}
          loading={!foreign && !current}
          error={current?.error}
        />
        <HistoryStrip
          key={`${key}:${revision}:outputs`}
          title="Output dataset checks"
          points={outputs}
          loading={!foreign && !current}
        />
        <HistoryStrip
          key={`${key}:${revision}:inputs`}
          title="Input dataset checks"
          points={current?.inputs ?? []}
          loading={!foreign && !current}
        />
        {current?.limited && (
          <p className="muted">
            History is limited to the latest 1,000 build jobs.
          </p>
        )}
        {foreign && (
          <p className="muted">
            Provider build and input-check history is unavailable. Output
            certificates are retained metadata.
          </p>
        )}
      </div>
    </div>
  );
}
