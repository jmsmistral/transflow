import { useEffect, useState, type ReactNode } from "react";
import type {
  ApiLineageNodeV1,
  ApiOverlayFactsV1,
  ApiHistoryJobV1,
  ApiDatasetHistoryV1,
  ExecutionJsonV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button, DatasetHeader } from "./components";
import { Icon } from "./Icons";
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
  title,
  values,
  empty,
}: {
  title: string;
  values: readonly ExecutionJsonV1[] | undefined;
  empty: string;
}) {
  const [limit, setLimit] = useState(20);
  return (
    <section className="health-section">
      <h3>{title}</h3>
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
            const outcome =
              field(check, "outcome") ??
              field(metrics, "status") ??
              "Unavailable";
            const name =
              field(metrics, "name") ??
              field(declaration, "name") ??
              field(check, "name") ??
              `Certificate ${index + 1}`;
            const severity =
              field(property(check, "policy"), "severity") ??
              field(metrics, "severity");
            const alias = field(check, "alias");
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
                  <strong>{name}</strong>
                  <span
                    className={`health-outcome outcome-${outcome.toLowerCase()}`}
                  >
                    <span aria-hidden="true">
                      {outcome === "PASS"
                        ? "✓"
                        : outcome === "VIOLATION"
                          ? "×"
                          : "!"}
                    </span>{" "}
                    {outcome}
                  </span>
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
    </section>
  );
}

function HistoryStrip({
  title,
  points,
  loading,
  error,
  children,
}: {
  title: string;
  points: readonly HealthPoint[];
  loading?: boolean;
  error?: string | undefined;
  children?: ReactNode;
}) {
  const latest = points[0];
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
            <li
              key={index}
              className={`health-dot health-${point?.tone ?? "neutral"}`}
              title={label}
              aria-label={`${index + 1}: ${label}`}
            >
              <span aria-hidden="true">
                {point?.tone === "good"
                  ? "✓"
                  : point?.tone === "warn"
                    ? "!"
                    : point?.tone === "bad"
                      ? "×"
                      : ""}
              </span>
            </li>
          );
        })}
      </ol>
      {error && (
        <p className="execution-error" role="alert">
          {error}
        </p>
      )}
      {children}
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
  const [revision, refresh] = useState(0);
  const [result, setResult] = useState<{
    key: string;
    revision: number;
    builds: HealthPoint[];
    outputs: HealthPoint[];
    inputs: HealthPoint[];
    outputChecks?: readonly ExecutionJsonV1[] | undefined;
    inputChecks?: readonly ExecutionJsonV1[] | undefined;
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
      const buildChecks = new Map<string, HealthPoint>();
      let inputChecks: readonly ExecutionJsonV1[] | undefined;
      for (const timing of timings.slice(0, 10)) {
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
          const checks = items(property(value.evidence, "checks")).filter(
            (c) => field(c, "phase") === "input",
          );
          if (!inputs.length) inputChecks = checks;
          if (!buildChecks.has(timing.owner.id))
            buildChecks.set(
              timing.owner.id,
              checkStatus(items(property(value.evidence, "checks"))),
            );
          inputs.push({ ...checkStatus(checks), at: timing.started_us });
        } catch (error) {
          if (abort.signal.aborted) return;
          errors.push(message(error));
          inputs.push({
            label: "Unavailable",
            tone: "neutral",
            at: timing.started_us,
          });
        }
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
      let outputChecks: readonly ExecutionJsonV1[] | undefined;
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
          if (!outputs.length) outputChecks = checks;
          outputs.push({ ...checkStatus(checks), at: publication.at });
        } catch (error) {
          if (abort.signal.aborted) return;
          errors.push(message(error));
          outputs.push({
            label: "Unavailable",
            tone: "neutral",
            at: publication.at,
          });
        }
      }
      if (!abort.signal.aborted)
        setResult({
          key,
          revision,
          builds: jobs.slice(0, 10).map((j) => ({
            ...(j.state === "SUCCEEDED" &&
            buildChecks.get(j.id)?.tone === "warn"
              ? { label: "Warning", tone: "warn" as const }
              : buildStatus(j.state)),
            at: j.created_us,
          })),
          outputs,
          inputs,
          outputChecks,
          inputChecks,
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
  const outputChecks = foreign
    ? Array.isArray(provider)
      ? provider
      : undefined
    : current?.outputChecks;
  const outputs = foreign
    ? outputChecks
      ? [checkStatus(outputChecks)]
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
      <DatasetHeader path={dataset.path}>
        <Button
          aria-label="Refresh check evidence"
          onClick={() => refresh((n) => n + 1)}
        >
          <Icon name="refresh" />
        </Button>
      </DatasetHeader>
      <div className="health-content">
        <HistoryStrip
          title="Latest builds"
          points={current?.builds ?? []}
          loading={!foreign && !current}
          error={current?.error}
        />
        <HistoryStrip
          title="Published data checks"
          points={outputs}
          loading={!foreign && !current}
        >
          {outputChecks && (
            <details>
              <summary>Latest check details</summary>
              <Checks
                title={
                  foreign ? "Provider output certificates" : "Output checks"
                }
                values={outputChecks}
                empty="No output checks were recorded for this version."
              />
            </details>
          )}
        </HistoryStrip>
        <HistoryStrip
          title="Input dataset checks"
          points={current?.inputs ?? []}
          loading={!foreign && !current}
        >
          {current?.inputChecks && (
            <details>
              <summary>Latest check details</summary>
              <Checks
                title="Input checks"
                values={current.inputChecks}
                empty="No input checks were recorded for this attempt."
              />
            </details>
          )}
        </HistoryStrip>
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
