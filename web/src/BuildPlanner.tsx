import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ApiContextV1,
  ApiLineageNodeV1,
  PlanResultV1,
  ApiAcceptedV1,
} from "./generated/contracts";
import { ApiFailure } from "./api/client";
import type { Workspace } from "./workspace";
import { type Ready, message } from "./execution-ui";
import { Button } from "./components";
import { Icon } from "./Icons";
import { BuildModal } from "./BuildReport";
import { useBuildEvidence, useBuildClock } from "./build-evidence";
import {
  type Options,
  selectedTargets,
  selection,
  resources,
} from "./planning";

interface Draft {
  plan: PlanResultV1;
  context: ApiContextV1;
  key: string;
  receipt: string;
}
interface Accepted {
  value: ApiAcceptedV1;
  plan: PlanResultV1;
}
export function BuildPlanner({
  workspace,
  state,
  selected,
  visible,
  onAdd,
  onSelect,
  onPlan,
}: {
  workspace: Workspace;
  state: Ready;
  selected: readonly ApiLineageNodeV1[];
  visible: readonly string[];
  onAdd: (paths: readonly string[]) => void;
  onSelect: (paths: readonly string[]) => void;
  onPlan: (plan: PlanResultV1 | null) => void;
}) {
  const targets = selectedTargets(selected);
  const [settings, setSettings] = useState<Omit<Options, "targets">>({
    mode: "selected",
    boundaries: "",
    exclusions: "",
    refresh: "",
    pins: "",
    parameters: "",
    force: false,
    requireCurrent: false,
    gitRef: "",
  });
  const options = useMemo(
    () => ({ ...settings, targets }),
    [settings, targets],
  );
  const [reviewing, setReviewing] = useState(false);
  const [showSkipped, setShowSkipped] = useState(true);
  const [autoPreview, setAutoPreview] = useState(false);
  const [draft, setDraft] = useState<Draft>();
  const [failedKey, setFailedKey] = useState<string>();
  const conflictRetries = useRef(0);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<"preview" | "accept" | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [rejected, setRejected] = useState(false);
  const [accepted, setAccepted] = useState<Accepted>();
  const [showBuild, setShowBuild] = useState(false);
  const previewRequest = useRef<AbortController | null>(null);
  const lifetime = useRef<AbortController | null>(null);
  useEffect(() => {
    lifetime.current = new AbortController();
    return () => {
      lifetime.current?.abort();
      previewRequest.current?.abort();
    };
  }, []);
  const key = JSON.stringify([options, state.value.context.fingerprint]);
  const latestKey = useRef(key);
  useEffect(() => {
    conflictRetries.current = 0;
    latestKey.current = key;
    previewRequest.current?.abort();
  }, [key]);
  const now = useBuildClock(!!draft);
  const expired = !!draft && now >= BigInt(draft.plan.expires_us);
  const stale = !!draft && draft.key !== key;
  const locked = busy === "accept" || uncertain;
  const preview = useCallback(async () => {
    previewRequest.current?.abort();
    const abort = new AbortController();
    previewRequest.current = abort;
    setAutoPreview(false);
    setFailedKey(undefined);
    setBusy("preview");
    setError("");
    setRejected(false);
    setReviewing(true);
    try {
      const request = selection(
        options,
        state.selection.branch,
        state.selection.fallback ?? null,
      );
      const result = await workspace.client.preparePlan(
        request,
        state.value.context,
        crypto.randomUUID(),
        abort.signal,
      );
      if (!abort.signal.aborted && latestKey.current === key) {
        conflictRetries.current = 0;
        setDraft({ ...result, key, receipt: crypto.randomUUID() });
      }
    } catch (e) {
      if (!abort.signal.aborted) {
        setFailedKey(key);
        setRejected(true);
        if (
          e instanceof ApiFailure &&
          e.kind === "conflict" &&
          conflictRetries.current < 3
        ) {
          conflictRetries.current += 1;
          await new Promise<void>((resolve) => {
            const timer = setTimeout(resolve, 500 * conflictRetries.current);
            abort.signal.addEventListener(
              "abort",
              () => {
                clearTimeout(timer);
                resolve();
              },
              { once: true },
            );
          });
          if (!abort.signal.aborted) await workspace.refresh(true);
          if (!abort.signal.aborted) setAutoPreview(true);
        } else {
          setError(message(e));
        }
      }
    } finally {
      if (previewRequest.current === abort) setBusy(null);
    }
  }, [
    options,
    state.selection.branch,
    state.selection.fallback,
    state.value.context,
    workspace,
    key,
  ]);
  useEffect(() => {
    if (!reviewing || !targets || locked || busy || state.refreshing) return;
    if (
      !autoPreview &&
      (failedKey === key || (draft && !stale && !expired && !rejected))
    )
      return;
    const timer = setTimeout(() => void preview(), 0);
    return () => clearTimeout(timer);
  }, [
    reviewing,
    targets,
    locked,
    busy,
    state.refreshing,
    autoPreview,
    failedKey,
    key,
    draft,
    stale,
    expired,
    rejected,
    preview,
  ]);
  async function accept() {
    if (
      !draft ||
      !lifetime.current ||
      busy ||
      (!uncertain && (stale || expired || rejected))
    )
      return;
    setBusy("accept");
    setError("");
    try {
      const value = await workspace.client.acceptPlan(
        draft.plan,
        draft.context,
        draft.receipt,
        lifetime.current.signal,
      );
      setAccepted({ value, plan: draft.plan });
      setDraft(undefined);
      setUncertain(false);
      setSettings((s) => ({ ...s, force: false }));
      setReviewing(false);
      setShowBuild(true);
    } catch (e) {
      if (lifetime.current.signal.aborted) return;
      const ambiguous =
        !(e instanceof ApiFailure) ||
        (e.kind !== "conflict" && e.kind !== "expired");
      setUncertain(ambiguous);
      setRejected(!ambiguous);
      setError(
        ambiguous
          ? "The build response was interrupted. Retry to recover the same build receipt."
          : "",
      );
    } finally {
      setBusy(null);
    }
  }
  function cancel() {
    previewRequest.current?.abort();
    setDraft(undefined);
    setReviewing(false);
    setAutoPreview(false);
    setFailedKey(undefined);
    setError("");
    setRejected(false);
  }
  const plan = !stale && !expired && !rejected ? draft?.plan : undefined;
  useEffect(() => {
    onPlan(
      reviewing && !!targets && plan && !stale && !expired && !rejected && !busy
        ? plan
        : null,
    );
    return () => onPlan(null);
  }, [onPlan, reviewing, targets, plan, stale, expired, rejected, busy]);
  const relevant = plan ? resources(plan) : [];
  const hidden = relevant.filter(
    (r) => !r.pending && !visible.includes(r.identity),
  );
  const count = targets ? targets.split("\n").length : 0;
  const skipped = plan
    ? relevant.filter(
        (r) =>
          !plan.writes.some(
            (w) => r.identity === `dataset:${plan.workspace}:${w.dataset}`,
          ),
      )
    : [];
  return (
    <div className="build-planner">
      {accepted && (
        <div className="planner-receipt">
          <Button
            disabled={accepted.plan.branch !== state.selection.branch}
            onClick={() => setShowBuild(true)}
          >
            View latest build
          </Button>
        </div>
      )}
      {!count && !locked ? (
        <div className="planner-empty" role="status">
          <Icon name="build" />
          <p>Select valid resources to begin a build</p>
          <small>Select datasets with local producers on the lineage.</small>
        </div>
      ) : (
        <>
          <div className="planner-count">
            {reviewing && plan
              ? `${plan.writes.length} ${plan.writes.length === 1 ? "resource" : "resources"} in build`
              : `${count} ${count === 1 ? "resource" : "resources"} selected`}
          </div>
          <div className="planner-heading">
            <strong>Builds</strong>
            <span className="muted">
              {reviewing ? "Finalize build options" : "Select a build strategy"}
            </span>
          </div>
          {!reviewing ? (
            <>
              <fieldset
                className="planner-strategies"
                disabled={locked}
                aria-label="Build strategy"
              >
                {(
                  [
                    ["selected", "Selected resources only"],
                    [
                      "connecting",
                      "All transforms in between selected resources",
                    ],
                    ["full", "All ancestor resources"],
                  ] as const
                ).map(([value, label]) => (
                  <label key={value}>
                    <input
                      type="radio"
                      name="build-strategy"
                      value={value}
                      checked={options.mode === value}
                      onChange={() =>
                        setSettings((s) => ({ ...s, mode: value }))
                      }
                    />
                    {label}
                  </label>
                ))}
              </fieldset>
              {selected.length > count && (
                <p className="muted">
                  {selected.length - count} selected resources have no local
                  producer and cannot be built here.
                </p>
              )}
              <footer className="planner-actions">
                <Button
                  className="button-primary"
                  disabled={!count || locked}
                  onClick={() => void preview()}
                >
                  Next (View preview)
                </Button>
              </footer>
            </>
          ) : (
            <>
              <fieldset
                disabled={locked || busy === "preview"}
                className="planner-toggles"
              >
                <label className="planner-check">
                  <input
                    type="checkbox"
                    role="switch"
                    checked={showSkipped}
                    onChange={(e) => setShowSkipped(e.target.checked)}
                  />
                  Show resources that will not be built
                </label>
                <label className="planner-check">
                  <input
                    type="checkbox"
                    role="switch"
                    checked={options.force}
                    onChange={(e) => {
                      setSettings((s) => ({ ...s, force: e.target.checked }));
                      setAutoPreview(true);
                    }}
                  />
                  Force build on up-to-date resources{" "}
                  <span title="Forces materialization within this scope. All checks still apply.">
                    <Icon name="info" />
                  </span>
                </label>
              </fieldset>
              {busy === "preview" && <p role="status">Preparing preview…</p>}
              {plan && (
                <section className="plan-review" aria-label="Plan preview">
                  <h3>Resources to be built</h3>
                  <p className="muted">Faded resources will not be built.</p>
                  <div className="planner-resource-list">
                    {plan.writes.map((w) => (
                      <div className="planner-resource" key={w.job}>
                        <Icon name="table" />
                        <div>
                          <strong>{w.path}</strong>
                          <small>
                            {plan.force
                              ? "Forced build"
                              : w.declaration?.cache === "deterministic"
                                ? "Cache evaluation pending"
                                : "Planned job"}
                          </small>
                        </div>
                      </div>
                    ))}
                    {showSkipped &&
                      skipped.map((r) => (
                        <div
                          className="planner-resource planner-skipped"
                          key={r.identity}
                        >
                          <Icon name="table" />
                          <div>
                            <strong>{r.path}</strong>
                            <small>Published input · will not be built</small>
                          </div>
                        </div>
                      ))}
                  </div>
                  {!!hidden.length && (
                    <div className="planner-notice">
                      <Icon name="info" />
                      <span>
                        {hidden.length} relevant{" "}
                        {hidden.length === 1 ? "resource is" : "resources are"}{" "}
                        outside the displayed lineage.
                      </span>
                      <Button
                        onClick={() =>
                          onAdd(hidden.slice(0, 100).map((r) => r.path))
                        }
                      >
                        Add to graph
                      </Button>
                    </div>
                  )}
                  <div className="planner-select-graph">
                    <Button
                      disabled={locked || !!busy || stale}
                      onClick={() => {
                        onSelect(plan.writes.slice(0, 100).map((w) => w.path));
                        cancel();
                      }}
                    >
                      Select nodes on graph
                      {plan.writes.length > 100 ? " (first 100)" : ""}
                    </Button>
                  </div>
                  <details className="planner-evidence">
                    <summary>Plan details</summary>
                    <p>
                      Branch {plan.branch} · {plan.writes.length} jobs ·{" "}
                      {plan.reads.length} published inputs
                    </p>
                    <p className="muted">
                      Deterministic jobs check exact inputs and retained
                      artifact bytes before executing; cache eligibility can
                      remain pending until parent jobs finish.
                    </p>
                    {plan.warnings.map((w, i) => (
                      <p key={i}>{w}</p>
                    ))}
                    <h4>Expected writes</h4>
                    {plan.writes.map((write) => (
                      <details className="planner-job" key={write.job}>
                        <summary>
                          <strong>{write.path}</strong>
                          <span className="muted">
                            {plan.force ||
                            write.declaration?.cache !== "deterministic"
                              ? "Planned job"
                              : "Cache evaluation pending"}
                          </span>
                        </summary>
                        {write.bindings.map((b) => (
                          <p key={b.alias}>
                            <strong>{b.alias}</strong>:{" "}
                            {b.kind === "planned"
                              ? "Output of a planned job"
                              : "Published input"}{" "}
                            ·{" "}
                            {plan.writes.find((w) => w.dataset === b.dataset)
                              ?.path ??
                              plan.reads.find((r) => r.dataset === b.dataset)
                                ?.path ??
                              b.dataset}
                          </p>
                        ))}
                        {write.declaration && (
                          <>
                            <p>
                              Cache: {write.declaration.cache ?? "default"} ·{" "}
                              {write.declaration.engine}
                            </p>
                            <h4>Checks</h4>
                            {[
                              ...write.declaration.inputs.flatMap((input) =>
                                input.checks.map((c) => ({
                                  ...c,
                                  label: `Input ${input.alias}`,
                                })),
                              ),
                              ...write.declaration.output.checks.map((c) => ({
                                ...c,
                                label: "Output",
                              })),
                            ].map((c, i) => (
                              <p key={i}>
                                {c.label} · {c.name} · {c.on_error}
                              </p>
                            ))}
                            {!write.declaration.output.checks.length &&
                              !write.declaration.inputs.some(
                                (i) => i.checks.length,
                              ) && (
                                <p className="muted">No declared data checks</p>
                              )}
                          </>
                        )}
                        {!!Object.keys(write.parameters).length && (
                          <>
                            <h4>Parameters</h4>
                            <pre>
                              {JSON.stringify(write.parameters, null, 2)}
                            </pre>
                          </>
                        )}
                        <h4>Resources</h4>
                        <p>
                          Execution limit: {write.resources.timeout_seconds}s ·
                          Validation limit:{" "}
                          {write.resources.validation_timeout_seconds}s
                        </p>
                        <p>
                          {write.resources.worker_threads} worker threads ·{" "}
                          {write.resources.memory_budget_mib
                            ? `${write.resources.memory_budget_mib} MiB reservation`
                            : "No Transflow memory cap"}
                        </p>
                      </details>
                    ))}
                    {!!plan.reads.length && (
                      <>
                        <h4>Published inputs</h4>
                        {plan.reads.map((r) => (
                          <div
                            className="planner-job"
                            key={`${r.consumer}:${r.alias}`}
                          >
                            <strong>{r.path}</strong>
                            <p>
                              {r.consumer_path} · {r.alias}
                            </p>
                            <p>
                              {r.origin_workspace &&
                              r.origin_workspace !== plan.workspace
                                ? "External provider · "
                                : ""}
                              {r.resolved_branch} · {r.resolution}
                            </p>
                            <p className="muted">Version {r.version}</p>
                          </div>
                        ))}
                      </>
                    )}
                    {!!plan.pending_registrations.length && (
                      <>
                        <h4>Pending registrations</h4>
                        <p className="muted">
                          Added only when this plan is accepted.
                        </p>
                        {plan.pending_registrations.map((r) => (
                          <p key={r.dataset}>{r.path}</p>
                        ))}
                      </>
                    )}
                    {!!Object.keys(plan.source_decisions ?? {}).length && (
                      <>
                        <h4>Source refresh</h4>
                        {Object.entries(plan.source_decisions ?? {}).map(
                          ([path, d]) => (
                            <p key={path}>
                              {plan.writes.find((w) => w.dataset === path)
                                ?.path ??
                                plan.reads.find((r) => r.dataset === path)
                                  ?.path ??
                                path}
                              : {d.reason} · {d.executes ? "refresh" : "reuse"}
                            </p>
                          ),
                        )}
                      </>
                    )}
                    {!!plan.freshness.length && (
                      <details>
                        <summary>Freshness and reasons</summary>
                        {plan.freshness.map((f) => (
                          <div className="planner-job" key={f.identity}>
                            <strong>{f.path ?? f.identity}</strong>
                            <p>
                              Data: {f.direct_data} · Logic: {f.direct_logic} ·
                              Ancestors: {f.inherited}
                            </p>
                            {f.reasons.map((r, i) => (
                              <p key={i}>{r.message}</p>
                            ))}
                          </div>
                        ))}
                      </details>
                    )}
                  </details>
                </section>
              )}
              <footer className="planner-actions">
                <Button disabled={locked} onClick={cancel}>
                  Cancel
                </Button>
                {error && failedKey === key && !uncertain && (
                  <Button
                    disabled={!!busy || !count}
                    onClick={() => void preview()}
                  >
                    Retry preview
                  </Button>
                )}
                <Button
                  className="button-primary"
                  disabled={
                    !draft ||
                    !plan?.writes.length ||
                    !!busy ||
                    (!uncertain && (stale || expired || rejected))
                  }
                  onClick={() => void accept()}
                >
                  <Icon name="build" />
                  {busy === "accept"
                    ? "Starting…"
                    : uncertain
                      ? "Retry build acceptance"
                      : "Run build"}
                </Button>
              </footer>
            </>
          )}
          {error && (
            <p className="planner-error" role="alert">
              {error}
            </p>
          )}
        </>
      )}
      {accepted &&
        showBuild &&
        accepted.plan.branch === state.selection.branch && (
          <AcceptedBuild
            workspace={workspace}
            state={state}
            accepted={accepted}
            onClose={() => setShowBuild(false)}
          />
        )}
    </div>
  );
}
function AcceptedBuild({
  workspace,
  state,
  accepted,
  onClose,
}: {
  workspace: Workspace;
  state: Ready;
  accepted: Accepted;
  onClose: () => void;
}) {
  const evidence = useBuildEvidence(workspace, state, {
    build: accepted.value.build,
    plan: accepted.value.plan,
    source: accepted.plan.source,
    build_state: accepted.value.state,
  });
  return (
    <BuildModal
      {...evidence}
      dataset={{ path: accepted.plan.targets.join(", ") }}
      onClose={onClose}
    />
  );
}
