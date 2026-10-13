import { useEffect, useState } from "react";
import type {
  ApiAttemptV1,
  ApiLineageNodeV1,
  ApiOverlayFactsV1,
  ExecutionJsonV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { Icon } from "./Icons";
import { field, versionMetadata } from "./inspectors";
import { timestamp } from "./execution-model";
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

export function HealthInspector({
  workspace,
  state,
  node,
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
  const [published, setPublished] = useState<{
    version: string;
    value: ApiAttemptV1;
  }>();
  const [recent, setRecent] = useState<{ key: string; value: ApiAttemptV1 }>();
  const [outputError, setOutputError] = useState("");
  const [recentError, setRecentError] = useState("");
  const [revision, refresh] = useState(0);
  const key = state.value.context.fingerprint;
  useEffect(() => {
    if (
      !dataset ||
      dataset.origin === "external" ||
      !meta?.version ||
      !meta.attempt
    )
      return;
    const abort = new AbortController();
    const version = meta.version,
      attempt = meta.attempt;
    void (async () => {
      const scope = await workspace.scope(
        {
          branch: state.selection.branch,
          dataset: dataset.dataset_id,
          origin: dataset.workspace_id,
          version,
        },
        abort.signal,
      );
      const value = await scope.read(
        "ApiAttemptV1",
        `/api/v1/attempts/${attempt}`,
      );
      if (value.attempt !== attempt || value.dataset !== dataset.dataset_id)
        throw new Error(
          "Published check evidence does not match this dataset.",
        );
      if (!abort.signal.aborted) {
        setPublished({ version, value });
        setOutputError("");
      }
    })().catch((failure) => {
      if (!abort.signal.aborted) setOutputError(message(failure));
    });
    return () => abort.abort();
  }, [workspace, state, dataset, meta?.version, meta?.attempt, revision]);
  useEffect(() => {
    if (!dataset || dataset.origin === "external") return;
    const abort = new AbortController();
    void (async () => {
      const scope = await workspace.scope(branchSelection(state), abort.signal);
      const history = await scope.read(
        "ApiDatasetHistoryV1",
        `/api/v1/datasets/${dataset.dataset_id}/history`,
        { limit: "50" },
      );
      const attempts = history.entries.flatMap((job) =>
        job.attempts.map((timing) => ({ ...timing, owner: job })),
      );
      attempts.sort((a, b) =>
        BigInt(a.started_us) > BigInt(b.started_us)
          ? -1
          : BigInt(a.started_us) < BigInt(b.started_us)
            ? 1
            : b.attempt.localeCompare(a.attempt),
      );
      const attempt = attempts[0];
      if (!attempt) return;
      const retained = await workspace.scope(
        { branch: state.selection.branch, plan: attempt.owner.plan },
        abort.signal,
      );
      const value = await retained.read(
        "ApiAttemptV1",
        `/api/v1/attempts/${attempt.attempt}`,
      );
      if (
        value.attempt !== attempt.attempt ||
        value.dataset !== dataset.dataset_id ||
        value.plan !== attempt.owner.plan ||
        value.build !== attempt.owner.build ||
        value.source !== attempt.owner.source
      )
        throw new Error("Recent check evidence does not match this dataset.");
      if (!abort.signal.aborted) {
        setRecent({ key, value });
        setRecentError("");
      }
    })().catch((failure) => {
      if (!abort.signal.aborted) setRecentError(message(failure));
    });
    return () => abort.abort();
  }, [workspace, state, dataset, key, revision]);
  if (!dataset) return <Empty>Select a dataset to inspect its health.</Empty>;
  const entry = state.value.versions?.entries.find(
    (value) => value.version === meta?.version,
  );
  const output =
    published && published.version === meta?.version
      ? published.value
      : undefined;
  const attempted = recent?.key === key ? recent.value : undefined;
  const provider =
    entry?.origin === "external"
      ? property(entry.metadata, "output_certificates")
      : undefined;
  const outputChecks =
    dataset.origin === "external"
      ? Array.isArray(provider)
        ? provider
        : undefined
      : output
        ? items(property(output.evidence, "checks")).filter(
            (check) => field(check, "phase") === "output",
          )
        : undefined;
  const inputs = attempted
    ? items(property(attempted.evidence, "checks")).filter(
        (check) => field(check, "phase") === "input",
      )
    : undefined;
  const overlay =
    node?.identity === `dataset:${dataset.workspace_id}:${dataset.dataset_id}`
      ? node.overlay
      : undefined;
  return (
    <div className="health-inspector">
      {!state.selection.version && (
        <Fallback
          branch={meta?.originBranch ?? field(dataset.head, "resolved_branch")}
          requested={state.selection.branch}
        />
      )}
      <div className="execution-toolbar">
        <strong>{dataset.path}</strong>
        <span className="muted">Data health</span>
        <div className="execution-switches">
          <Button
            aria-label="Refresh check evidence"
            onClick={() => refresh((n) => n + 1)}
          >
            <Icon name="refresh" />
          </Button>
        </div>
      </div>
      <section className="health-section">
        <dl className="health-summary">
          <div>
            <dt>Published output checks</dt>
            <dd>
              {meta?.version && overlay?.version === meta.version
                ? healthLabel(overlay.health)
                : "Unavailable"}
            </dd>
          </div>
          <div>
            <dt>Latest attempt on {state.selection.branch}</dt>
            <dd>{overlay?.latest_attempt ?? "Unavailable"}</dd>
          </div>
        </dl>
        {overlay?.input_failed && (
          <p className="health-input-failure" role="status">
            The latest consumer input check failed. Published output checks are
            shown separately.
          </p>
        )}
        {!meta?.version && (
          <Empty>No published version is available on this branch.</Empty>
        )}
      </section>
      {outputError && (
        <p className="execution-error" role="alert">
          Published check evidence: {outputError}
        </p>
      )}
      {dataset.kind === "imported" && !meta?.attempt ? (
        <section className="health-section">
          <h3>Published version checks</h3>
          <p>Imported data has no producer output checks.</p>
        </section>
      ) : (
        <Checks
          title={
            dataset.origin === "external"
              ? "Provider output certificates"
              : "Published version checks"
          }
          values={outputChecks}
          empty="No output checks were recorded for this version."
        />
      )}
      {dataset.origin !== "external" && (
        <>
          <section className="health-section">
            <h3>Recent attempted job</h3>
            {attempted ? (
              <p>
                {field(attempted.evidence, "state")} ·{" "}
                {timestamp(field(attempted.evidence, "started_us"), "minute")}
              </p>
            ) : (
              <p className="muted">Attempt details unavailable.</p>
            )}
            <small>
              Most recent attempt among the latest 50 build jobs. Cached reuse
              does not create an attempt.
            </small>
            {recentError && (
              <p className="execution-error" role="alert">
                {recentError}
              </p>
            )}
          </section>
          {attempted && attempted.attempt !== meta?.attempt && (
            <Checks
              title="Recent attempt output checks"
              values={items(property(attempted.evidence, "checks")).filter(
                (check) => field(check, "phase") === "output",
              )}
              empty="No output checks were recorded for this attempt."
            />
          )}
          <Checks
            title="Consumer input checks"
            values={inputs}
            empty="No input checks were recorded for this attempt."
          />
        </>
      )}
    </div>
  );
}
