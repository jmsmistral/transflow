import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type {
  ApiDatasetV1,
  ApiLineageNodeV1,
  ApiScheduleRolesV1,
  ScheduleRecordV1,
  ScheduleDefinitionV1,
  ApiScheduleHistoryV1,
  ApiScheduleMetricsV1,
  ScheduleHistoryEntryV1,
  PlanResultV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { visualContext } from "./workspace";
import { Button } from "./components";
import { Icon } from "./Icons";
import { ApiFailure } from "./api/client";
import {
  type Ready,
  Empty,
  Evidence,
  Metric,
  Status,
  message,
} from "./execution-ui";
import {
  savedDefinition,
  related,
  identity,
  keyOf,
  minute,
  duration,
  ratio,
} from "./schedule-model";
import type { ScheduleEditSession } from "./schedule-model";
import { ScheduleEditor, Switch } from "./ScheduleEditor";
import { cronDescription } from "./cron-description";
import { historyDateRange, percent, position } from "./execution-model";
import { useBuildEvidence } from "./build-evidence";
import { BuildModal } from "./BuildReport";
import {
  ScheduleBranches,
  ScheduleState,
  ScheduleBuildScope,
  ScheduleDatasets,
  ScheduleFacts,
  type SchedulePath,
} from "./ScheduleDetails";

function ScheduleModal({
  workspace,
  state,
  record,
  definition,
  path,
  actions,
  scopePlan,
  scopeError,
  onClose,
}: {
  workspace: Workspace;
  state: Ready;
  record: ScheduleRecordV1;
  definition: ScheduleDefinitionV1 | null;
  path: SchedulePath;
  actions: ReactNode;
  scopePlan: PlanResultV1 | null;
  scopeError: string;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const title = useId();
  useEffect(() => {
    const previous = document.activeElement;
    const dialog = ref.current;
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
      className="build-modal schedule-modal"
      aria-labelledby={title}
      onCancel={(event) => {
        // Escape belongs to the topmost dialog when a retained Build report is open.
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <header className="build-modal-header">
        <Icon name="calendar" />
        <h2 id={title}>Schedule details</h2>
        <span>{definition?.name ?? "Schedule needs review"}</span>
        <Button aria-label="Close schedule view" onClick={onClose}>
          <Icon name="close" />
        </Button>
      </header>
      <div className="schedule-modal-content schedules">
        <aside className="schedule-modal-definition">
          <header className="schedule-heading">
            <h3>{definition?.name ?? "Schedule needs review"}</h3>
            <ScheduleState record={record} />
          </header>
          {actions}
          {definition && (
            <>
              {definition.description && <p>{definition.description}</p>}
              <ScheduleBuildScope plan={scopePlan} error={scopeError} />
              <h4>When to build</h4>
              <TriggerSummary trigger={definition.trigger} paths={path} />
              <h4>Branches</h4>
              <ScheduleBranches definition={definition} />
              <h4>Code and build options</h4>
              <ScheduleFacts definition={definition} full />
              <h4>Read boundaries</h4>
              <ScheduleDatasets
                datasets={definition.build.boundaries}
                path={path}
              />
              <h4>Excluded datasets</h4>
              <ScheduleDatasets
                datasets={definition.build.exclusions}
                path={path}
              />
              <h4>Refresh sources</h4>
              <ScheduleDatasets
                datasets={definition.build.refresh_sources}
                path={path}
              />
              <details>
                <summary>Input policies and parameters</summary>
                <Evidence>
                  {{
                    input_fallback_policy:
                      definition.build.input_fallback_policy,
                    provider_fallback_policies:
                      definition.build.provider_fallback_policies,
                    parameters: definition.build.parameters,
                  }}
                </Evidence>
              </details>
            </>
          )}
          <details>
            <summary>Saved definition</summary>
            <Evidence>{record.definition}</Evidence>
          </details>
        </aside>
        <main className="schedule-modal-history">
          <ScheduleHistory
            workspace={workspace}
            state={state}
            record={record}
          />
        </main>
      </div>
    </dialog>
  );
}

function ScheduleBuild({
  workspace,
  state,
  entry,
  onClose,
}: {
  workspace: Workspace;
  state: Ready;
  entry: ScheduleHistoryEntryV1;
  onClose: () => void;
}) {
  const selected = useMemo(
    () => ({
      ...state,
      selection: {
        ...state.selection,
        branch: entry.branch ?? state.selection.branch,
      },
    }),
    [state, entry.branch],
  );
  const job = {
    build: entry.build ?? "",
    plan: entry.plan ?? "",
    source: entry.source ?? "",
    build_state: entry.build_state ?? "QUEUED",
  };
  const evidence = useBuildEvidence(workspace, selected, job);
  return (
    <BuildModal
      {...evidence}
      dataset={{ path: "Scheduled datasets" }}
      onClose={onClose}
      onOpenSchedule={(id) => {
        onClose();
        workspace.requestSchedule(id);
      }}
    />
  );
}
function DeleteDialog({
  name,
  onClose,
  onDelete,
  busy,
}: {
  name: string;
  onClose: () => void;
  onDelete: () => void;
  busy: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const previous = document.activeElement;
    ref.current?.showModal();
    const dialog = ref.current;
    return () => {
      dialog?.close();
      if (previous instanceof HTMLElement && previous.isConnected)
        previous.focus();
    };
  }, []);
  return (
    <dialog
      ref={ref}
      className="schedule-delete"
      aria-label="Delete schedule"
      onCancel={onClose}
    >
      <h2>Delete {name}?</h2>
      <p>
        This removes the schedule from active lists and cancels unstarted
        automatic occurrences. History, manual requests and active builds
        remain.
      </p>
      <footer className="schedule-actions">
        <Button disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button className="button-primary" disabled={busy} onClick={onDelete}>
          Delete schedule
        </Button>
      </footer>
    </dialog>
  );
}
function TriggerSummary({
  trigger,
  paths,
  compact = false,
}: {
  compact?: boolean;
  trigger: ScheduleDefinitionV1["trigger"];
  paths: (key: import("./generated/contracts").DatasetKey) => string;
}) {
  switch (trigger.kind) {
    case "manual":
      return <ConditionCard icon="play">Manual only</ConditionCard>;
    case "and":
    case "or":
      return (
        <div className="schedule-trigger-summary">
          <strong>
            {trigger.kind === "and" ? "All conditions" : "Any condition"}
          </strong>
          <div className="schedule-conditions" role="list">
            {trigger.children.map((c, i) => (
              <div key={i} role="listitem">
                <TriggerSummary trigger={c} paths={paths} compact={compact} />
              </div>
            ))}
          </div>
        </div>
      );
    case "cron":
      return (
        <ConditionCard icon="calendar">
          {cronDescription(trigger.expression)} · {trigger.timezone}
          {!compact && (
            <small className="schedule-clock-policy">
              {trigger.duplicate_time === "both"
                ? "Run at both occurrences when clocks repeat."
                : "Run once at the first occurrence when clocks repeat."}
            </small>
          )}
        </ConditionCard>
      );
    case "dataset_published":
    case "dataset_head_changed":
      return (
        <ConditionCard icon="table">
          {paths(trigger.dataset)} ·{" "}
          {trigger.kind === "dataset_published" ? "published" : "head changed"}{" "}
          on {trigger.branch}
          {!compact &&
            ` · ${trigger.payload_mode === "pin" ? "exact version pin" : "signal only"}`}
          {trigger.include_resets ? " · includes resets" : ""}
        </ConditionCard>
      );
    case "schedule_succeeded":
      return (
        <ConditionCard icon="calendar">
          Schedule {trigger.schedule_id} succeeded
          {trigger.require_materialization ? " with materialization" : ""}
        </ConditionCard>
      );
    case "build_succeeded":
      return (
        <ConditionCard icon="build">
          Build of {trigger.targets.map(paths).join(", ")} succeeded on{" "}
          {trigger.branch}
        </ConditionCard>
      );
  }
}
function ConditionCard({
  icon,
  children,
}: {
  icon: "calendar" | "table" | "build" | "play";
  children: ReactNode;
}) {
  return (
    <div className="schedule-condition-card">
      <Icon name={icon} />
      <span>{children}</span>
    </div>
  );
}
function ScheduleHistory({
  workspace,
  state,
  record,
}: {
  workspace: Workspace;
  state: Ready;
  record: ScheduleRecordV1;
}) {
  const [today] = useState(() => new Date().toISOString().slice(0, 10));
  const [month] = useState(() =>
    new Date(Date.now() - 30 * 86400000).toISOString().slice(0, 10),
  );
  const [from, setFrom] = useState(month),
    [to, setTo] = useState(today),
    [range, setRange] = useState(() => historyDateRange(month, today));
  const [history, setHistory] = useState<ApiScheduleHistoryV1 | null>(null),
    [metrics, setMetrics] = useState<ApiScheduleMetricsV1 | null>(null),
    [error, setError] = useState("");
  const [page, setPage] = useState<string | null>(null),
    [revision, setRevision] = useState(0),
    [build, setBuild] = useState<ScheduleHistoryEntryV1 | null>(null);
  const contextKey = visualContext(state.value.context);
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function read() {
      try {
        const [h, m] = await Promise.all([
          workspace.client.read(
            "ApiScheduleHistoryV1",
            `/api/v1/schedules/${record.id}/history`,
            { limit: "100", ...(page ? { after: page } : {}) },
            abort.signal,
          ),
          range
            ? workspace.client.read(
                "ApiScheduleMetricsV1",
                `/api/v1/schedules/${record.id}/metrics`,
                range,
                abort.signal,
              )
            : Promise.resolve(null),
        ]);
        if (!abort.signal.aborted) {
          setHistory(h.data);
          setMetrics(m?.data ?? null);
          setError("");
        }
      } catch (e) {
        if (!abort.signal.aborted) setError(message(e));
      }
      if (!abort.signal.aborted)
        timer = setTimeout(() => {
          void read();
        }, 2000);
    }
    void read();
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
  }, [workspace, record.id, contextKey, page, range, revision]);
  const samples =
    history?.occurrences.filter(
      (e) =>
        e.wall_duration_us !== null &&
        range &&
        BigInt(e.ready_us) >= BigInt(range.from_us) &&
        BigInt(e.ready_us) < BigInt(range.to_us),
    ) ?? [];
  const maximum = samples.reduce(
    (max, e) =>
      BigInt(e.wall_duration_us ?? "0") > max
        ? BigInt(e.wall_duration_us ?? "0")
        : max,
    1n,
  );
  return (
    <section className="schedule-history">
      <header className="schedule-summary">
        <h3>Summary</h3>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            const next = historyDateRange(from, to);
            if (next) {
              setRange(next);
              setError("");
            } else setError("Choose a valid inclusive date range");
          }}
        >
          <label>
            From
            <input
              type="text"
              placeholder="YYYY-MM-DD"
              pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}"
              title="YYYY-MM-DD"
              required
              value={from}
              onChange={(e) => setFrom(e.target.value)}
            />
          </label>
          <label>
            To
            <input
              type="text"
              placeholder="YYYY-MM-DD"
              pattern="[0-9]{4}-[0-9]{2}-[0-9]{2}"
              title="YYYY-MM-DD"
              required
              value={to}
              onChange={(e) => setTo(e.target.value)}
            />
          </label>
          <Button type="submit">Apply</Button>
        </form>
      </header>
      {metrics && (
        <>
          <div className="metrics-summary">
            <Metric label="Occurrences" value={metrics.occurrences} />
            <Metric label="Builds" value={metrics.builds} />
            <Metric
              label="Jobs / attempts"
              value={`${metrics.jobs} / ${metrics.attempts}`}
            />
            <Metric
              label="Failure rate"
              value={percent(metrics.failure_rate)}
              title="Failed / succeeded plus failed builds"
            />
            <Metric
              label="Mean · latest 10 successes"
              value={ratio(metrics.trailing_mean_us)}
            />
            <Metric
              label="Median successful operation time"
              value={ratio(metrics.median_us)}
            />
            <Metric
              label="Measured / missing"
              value={`${metrics.duration_samples} / ${metrics.missing_duration_samples}`}
            />
          </div>
          <p className="muted">
            Occurrences ready in the selected dates. Operation duration includes
            cached reuse.{" "}
            {metrics.dispositions
              .map((c) => `${c.count} ${c.state.toLowerCase()}`)
              .join(" · ")}
          </p>
          <p className="muted">
            {metrics.ignored_event_matches} ignored event matches observed in
            this window · Clock ticks by intended time:{" "}
            {metrics.clock_ticks
              .map((c) => `${c.count} ${c.state.toLowerCase()}`)
              .join(" · ") || "none"}
          </p>
        </>
      )}
      {range && samples.length > 0 ? (
        <svg
          className="schedule-chart"
          viewBox="0 0 640 210"
          role="img"
          aria-label="Schedule build operation durations"
        >
          <line x1="60" y1="20" x2="60" y2="170" />
          <line x1="60" y1="170" x2="620" y2="170" />
          <text x="4" y="25">
            {duration(maximum.toString())}
          </text>
          <text x="15" y="174">
            0 s
          </text>
          {samples.map((e) => (
            <circle
              key={e.id}
              tabIndex={0}
              cx={
                60 +
                position(
                  BigInt(e.ready_us),
                  BigInt(range.from_us),
                  BigInt(range.to_us) - BigInt(range.from_us),
                ) *
                  5.6
              }
              cy={
                170 -
                position(BigInt(e.wall_duration_us ?? "0"), 0n, maximum) * 1.5
              }
              r="4"
              className={`status-${(e.build_state ?? e.disposition).toLowerCase()}`}
            >
              <title>
                {e.build_state ?? e.disposition} · {minute(e.ready_us)} ·{" "}
                {duration(e.wall_duration_us)}
              </title>
            </circle>
          ))}
          <text x="60" y="200">
            {from}
          </text>
          <text x="542" y="200">
            {to}
          </text>
        </svg>
      ) : (
        <Empty>
          No measured build operations in this history page and date range.
        </Empty>
      )}
      <h3>Recent builds and occurrences</h3>
      <p className="muted">
        Newest first · operation durations ·{" "}
        {page ? "older page" : "latest page"}
      </p>
      {history?.occurrences.map((e) => (
        <article key={e.id} className="schedule-occurrence">
          <Status state={e.build_state ?? e.disposition} />
          <div>
            <strong>{minute(e.ready_us)}</strong>
            <p>
              {e.build_state ?? e.disposition} ·{" "}
              {e.manual === true
                ? "Manual"
                : e.manual === false
                  ? "Automatic"
                  : "Origin unavailable"}{" "}
              · {e.jobs} {e.jobs === "1" ? "job" : "jobs"}
            </p>
            <small>
              {e.branch ?? "No accepted branch"} · {e.disposition.toLowerCase()}
              {e.coalesced_tokens !== null
                ? ` · ${e.coalesced_tokens} replaced tokens`
                : ""}
            </small>
          </div>
          <span>{duration(e.wall_duration_us)}</span>
          {e.build && e.plan && e.source && (
            <Button onClick={() => setBuild(e)}>View build</Button>
          )}
        </article>
      ))}
      {history?.occurrences.length === 0 && <Empty>No occurrences yet.</Empty>}
      <div className="schedule-actions">
        {page && <Button onClick={() => setPage(null)}>Latest</Button>}
        {history?.next_cursor && (
          <Button onClick={() => setPage(history.next_cursor)}>
            Older occurrences
          </Button>
        )}
        <Button
          aria-label="Refresh schedule history"
          onClick={() => setRevision((n) => n + 1)}
        >
          <Icon name="refresh" />
        </Button>
      </div>
      {error && (
        <p className="execution-error" role="alert">
          {error}
        </p>
      )}
      {build && (
        <ScheduleBuild
          workspace={workspace}
          state={state}
          entry={build}
          onClose={() => setBuild(null)}
        />
      )}
    </section>
  );
}

export function Schedules({
  workspace,
  state,
  selected,
  visible,
  onRoles,
  onPlan,
  onAdd,
  focusRequest,
  onEdit,
  onActive,
}: {
  workspace: Workspace;
  state: Ready;
  selected: readonly ApiLineageNodeV1[];
  visible: readonly string[];
  onRoles: (roles: ApiScheduleRolesV1 | null) => void;
  onPlan: (plan: PlanResultV1 | null) => void;
  onAdd: (paths: readonly string[]) => void;
  focusRequest?: { id: string; revision: number } | null;
  onEdit?: (session: ScheduleEditSession | null) => void;
  onActive?: (active: boolean) => void;
}) {
  const [rows, setRows] = useState<readonly ScheduleRecordV1[]>([]),
    [next, setNext] = useState<string | null>(null),
    [all, setAll] = useState(false),
    [query, setQuery] = useState("");
  const [chosen, setChosen] = useState<string | null>(null),
    [draft, setDraft] = useState<{
      definition: ScheduleDefinitionV1;
      record: ScheduleRecordV1 | null;
    } | null>(null),
    [roles, setRoles] = useState<ApiScheduleRolesV1 | null>(null);
  const [roleError, setRoleError] = useState("");
  const [scope, setScope] = useState<{
    basis: string;
    plan: PlanResultV1 | null;
    error: string;
  } | null>(null);
  const [error, setError] = useState(""),
    [notice, setNotice] = useState(""),
    [busy, setBusy] = useState(false),
    [revision, setRevision] = useState(0),
    [deleting, setDeleting] = useState(false),
    [viewOpen, setViewOpen] = useState(false);
  const [datasets, setDatasets] = useState<readonly ApiDatasetV1[]>(
      state.value.datasets.entries,
    ),
    [catalogueCursor, setCatalogueCursor] = useState(
      state.value.datasets.next_cursor,
    );
  const pending = useRef<AbortController | null>(null);
  const context = state.value.context,
    contextKey = visualContext(context);
  const current = rows.find((r) => r.id === chosen) ?? null,
    definition = current ? savedDefinition(current) : null;
  useEffect(() => {
    onActive?.(!!chosen || !!draft);
  }, [onActive, chosen, draft]);
  useEffect(() => () => onActive?.(false), [onActive]);
  const scopeNodes = selected.length
    ? selected.map((n) => n.identity)
    : visible;
  const matches = rows.filter((r) => {
    const d = savedDefinition(r);
    return (
      (all || (d && related(d, scopeNodes).any)) &&
      (d?.name ?? "Schedule needs review")
        .toLowerCase()
        .includes(query.toLowerCase())
    );
  });
  useEffect(() => {
    if (!focusRequest) return;
    const abort = new AbortController();
    void workspace.client
      .read(
        "ScheduleRecordV1",
        `/api/v1/schedules/${encodeURIComponent(focusRequest.id)}`,
        {},
        abort.signal,
      )
      .then((r) => {
        if (!abort.signal.aborted) {
          setRows((previous) => [
            r.data,
            ...previous.filter((p) => p.id !== r.data.id),
          ]);
          setChosen(r.data.id);
          setViewOpen(true);
          onPlan(null);
        }
      })
      .catch((e) => {
        if (!abort.signal.aborted) setError(message(e));
      });
    return () => abort.abort();
  }, [focusRequest, workspace, onPlan]);
  useEffect(
    () => () => {
      pending.current?.abort();
      onRoles(null);
      onPlan(null);
    },
    [onRoles, onPlan],
  );
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function read() {
      try {
        const page = await workspace.client.read(
          "ApiSchedulesV1",
          "/api/v1/schedules",
          {},
          abort.signal,
        );
        if (!abort.signal.aborted) {
          setRows((previous) => [
            ...page.data.schedules,
            ...previous.filter(
              (p) => !!page.data.next_cursor && p.id > page.data.next_cursor,
            ),
          ]);
          setNext(page.data.next_cursor);
        }
      } catch (e) {
        if (!abort.signal.aborted) setError(message(e));
      }
      if (!abort.signal.aborted)
        timer = setTimeout(() => {
          void read();
        }, 2500);
    }
    void read();
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
  }, [workspace, contextKey, revision]);
  const currentId = current?.id;
  const currentEtag = current?.etag;
  const rolesBranch = context.branch;
  const rolesFallback = JSON.stringify(context.fallback_policy.slice(1));
  useEffect(() => {
    if (!currentId) {
      onRoles(null);
      return;
    }
    const abort = new AbortController();
    void (async () => {
      // Saved role facts do not depend on runtime status. Fence their source/policy
      // context and edit ETag without invalidating them on unrelated job transitions.
      const readPage = async (after?: string) => {
        const result = await workspace.client.read(
          "ApiScheduleRolesV1",
          `/api/v1/schedules/${currentId}/roles`,
          {
            branch: rolesBranch,
            fallback: rolesFallback,
            ...(after ? { after } : {}),
          },
          abort.signal,
        );
        if (!result.context || visualContext(result.context) !== contextKey)
          throw new DOMException(
            "Schedule source context changed",
            "AbortError",
          );
        if (result.data.etag !== currentEtag)
          throw Error("The saved schedule changed; updating its role facts");
        return result.data;
      };
      let page = await readPage();
      const first = page,
        nodes = [...page.nodes];
      let count = 0;
      while (page.next_cursor) {
        if (++count > 68)
          throw Error(
            "The saved schedule roles exceed the supported reading budget",
          );
        page = await readPage(page.next_cursor);
        nodes.push(...page.nodes);
      }
      return { data: { ...first, nodes, next_cursor: null } };
    })()
      .then((r) => {
        if (!abort.signal.aborted) {
          setRoles(r.data);
          setRoleError("");
          onRoles(r.data);
        }
      })
      .catch((e) => {
        if (!abort.signal.aborted) {
          setRoleError(
            e instanceof DOMException && e.name === "AbortError"
              ? "Updating schedule roles…"
              : message(e),
          );
          onRoles(null);
        }
      });
    return () => abort.abort();
  }, [
    workspace,
    currentId,
    currentEtag,
    rolesBranch,
    rolesFallback,
    contextKey,
    onRoles,
  ]);
  const scopeBasis =
    current && !draft
      ? JSON.stringify([current.id, current.etag, contextKey, revision])
      : "";
  const definitionJson = definition ? JSON.stringify(definition) : "";
  const scopeContext = useRef(context);
  useEffect(() => {
    scopeContext.current = context;
  }, [context]);
  useEffect(() => {
    onPlan(null);
    if (!scopeBasis || !definitionJson) return;
    const abort = new AbortController();
    // Use the same backend source, branches and scope adapter as dispatch.
    // Runtime status polling alone must not start a new planning request.
    const saved = JSON.parse(definitionJson) as ScheduleDefinitionV1;
    const prepare = async () => {
      let capturedContext = scopeContext.current;
      for (let attempt = 0; ; attempt++) {
        abort.signal.throwIfAborted();
        try {
          if (attempt > 0) {
            await new Promise<void>((resolve) => {
              const done = () => {
                clearTimeout(timer);
                abort.signal.removeEventListener("abort", done);
                resolve();
              };
              const timer = setTimeout(done, 500 * attempt);
              abort.signal.addEventListener("abort", done, { once: true });
            });
            abort.signal.throwIfAborted();
            await workspace.refresh(true);
            abort.signal.throwIfAborted();
            const refreshed = workspace.snapshot();
            if (
              refreshed.kind !== "ready" ||
              visualContext(refreshed.value.context) !== contextKey
            )
              throw new DOMException(
                "Schedule source context changed",
                "AbortError",
              );
            capturedContext = refreshed.value.context;
          }
          return await workspace.client.schedulePreview(
            saved,
            capturedContext,
            abort.signal,
          );
        } catch (e) {
          if (
            !(e instanceof ApiFailure) ||
            e.kind !== "conflict" ||
            attempt >= 2
          )
            throw e;
        }
      }
    };
    void prepare()
      .then((plan) => {
        if (!abort.signal.aborted) {
          setScope({ basis: scopeBasis, plan, error: "" });
          onPlan(plan);
        }
      })
      .catch((e) => {
        if (!abort.signal.aborted)
          setScope({
            basis: scopeBasis,
            plan: null,
            error: `Schedule build scope unavailable. ${message(e)}`,
          });
      });
    return () => abort.abort();
  }, [workspace, scopeBasis, definitionJson, contextKey, onPlan]);
  const scopePlan = scope?.basis === scopeBasis ? scope.plan : null;
  const scopeError = scope?.basis === scopeBasis ? scope.error : "";
  const defaults = async () => {
    const abort = new AbortController();
    pending.current?.abort();
    pending.current = abort;
    const definition = (
      await workspace.client.read(
        "ScheduleDefinitionV1",
        "/api/v1/schedules/defaults",
        {
          branch: context.branch,
          fallback: JSON.stringify(context.fallback_policy.slice(1)),
        },
        abort.signal,
        context,
      )
    ).data;
    return {
      ...definition,
      name: "",
      build: { ...definition.build, build_mode: "selected" as const },
    };
  };
  const path = (k: import("./generated/contracts").DatasetKey) =>
    datasets.find((d) => identity(keyOf(d)) === identity(k))?.path ??
    k.dataset_id;
  const action = async (kind: "pause" | "resume" | "run" | "delete") => {
    if (!current) return;
    const abort = new AbortController();
    pending.current?.abort();
    pending.current = abort;
    setBusy(true);
    setError("");
    try {
      if (kind === "run") {
        await workspace.client.scheduleAction(
          "ScheduleRunV1",
          current,
          kind,
          abort.signal,
        );
        if (!abort.signal.aborted)
          setNotice(
            "Manual occurrence queued. Automatic pause state is unchanged.",
          );
      } else if (kind === "delete") {
        await workspace.client.scheduleAction(
          "ApiScheduleDeletedV1",
          current,
          kind,
          abort.signal,
        );
        if (!abort.signal.aborted) {
          setRows(rows.filter((r) => r.id !== current.id));
          setChosen(null);
          setDeleting(false);
        }
      } else {
        const saved = await workspace.client.scheduleAction(
          "ScheduleRecordV1",
          current,
          kind,
          abort.signal,
        );
        if (!abort.signal.aborted)
          setRows(rows.map((r) => (r.id === saved.id ? saved : r)));
      }
      // Schedule writes advance the runtime read fence without a job event.
      // Refresh verified context before subsequent graph/branch metadata reads.
      if (!abort.signal.aborted) await workspace.refresh(true);
      if (!abort.signal.aborted) setRevision((n) => n + 1);
    } catch (e) {
      if (!abort.signal.aborted) setError(message(e));
    } finally {
      if (!abort.signal.aborted) setBusy(false);
    }
  };
  const relevant = new Map((roles?.nodes ?? []).map((n) => [n.identity, n]));
  for (const write of scopePlan?.writes ?? []) {
    const key = identity({
      workspace_id: scopePlan?.workspace ?? context.workspace,
      dataset_id: write.dataset,
    });
    if (!relevant.has(key))
      relevant.set(key, { identity: key, paths: [write.path], roles: [] });
  }
  const hidden = [...relevant.values()].filter(
    (n) => !visible.includes(n.identity),
  );
  const choose = (id: string) => {
    setChosen(id);
    setViewOpen(false);
    setRoles(null);
    onPlan(null);
    onRoles(null);
    setNotice("");
  };
  const scheduleActions = current ? (
    <div className="schedule-actions">
      <Button
        disabled={busy || current.needs_review}
        onClick={() => {
          void action("run");
        }}
      >
        Run now
      </Button>
      <Button
        disabled={busy || current.needs_review}
        onClick={() => {
          void action(current.paused ? "resume" : "pause");
        }}
      >
        {current.paused ? "Resume" : "Pause"}
      </Button>
      <Button
        disabled={busy}
        onClick={() => {
          setViewOpen(false);
          if (definition) setDraft({ definition, record: current });
          else
            void defaults()
              .then((d) => setDraft({ definition: d, record: current }))
              .catch((e) => setError(message(e)));
        }}
      >
        Edit
      </Button>
      <Button disabled={busy} onClick={() => setDeleting(true)}>
        Delete
      </Button>
    </div>
  ) : null;
  if (draft)
    return (
      <section className="schedules">
        <ScheduleEditor
          key={`${draft.record?.id ?? "new"}:${draft.record?.etag ?? ""}`}
          workspace={workspace}
          initial={draft.definition}
          record={draft.record}
          datasets={datasets}
          schedules={rows}
          selected={selected}
          {...(onEdit ? { onEdit } : {})}
          onReload={async (signal) => {
            if (!draft.record) return;
            const row = (
              await workspace.client.read(
                "ScheduleRecordV1",
                `/api/v1/schedules/${draft.record.id}`,
                {},
                signal,
              )
            ).data;
            if (signal.aborted) return;
            const definition = savedDefinition(row);
            if (!definition)
              throw Error("The saved schedule needs a new valid definition");
            setDraft({ definition, record: row });
            setRows((previous) => [
              row,
              ...previous.filter((r) => r.id !== row.id),
            ]);
          }}
          onCancel={() => {
            setDraft(null);
            setChosen(null);
            setNotice("");
            onPlan(null);
            onRoles(null);
          }}
          onSave={async (d, signal, exitToList) => {
            const saved = await workspace.client.saveSchedule(
              d,
              draft.record,
              signal,
            );
            if (signal.aborted) return;
            await workspace.refresh(true);
            if (signal.aborted) return;
            setRows((previous) => [
              saved,
              ...previous.filter((r) => r.id !== saved.id),
            ]);
            setChosen(exitToList ? null : saved.id);
            setDraft(null);
            setNotice("Schedule saved. Its current definition was replaced.");
            setRevision((n) => n + 1);
          }}
        />
        {catalogueCursor && (
          <Button
            onClick={() => {
              const abort = new AbortController();
              void workspace.client
                .read(
                  "ApiDatasetsV1",
                  "/api/v1/datasets",
                  {
                    branch: context.branch,
                    cursor: catalogueCursor,
                    limit: "200",
                  },
                  abort.signal,
                  context,
                )
                .then((r) => {
                  setDatasets([...datasets, ...r.data.entries]);
                  setCatalogueCursor(r.data.next_cursor);
                })
                .catch((e) => setError(message(e)));
            }}
          >
            Load more catalogue datasets
          </Button>
        )}
      </section>
    );
  return (
    <section className="schedules">
      {chosen && (
        <Button
          className="schedule-back schedule-back-top"
          onClick={() => {
            setViewOpen(false);
            setChosen(null);
            onPlan(null);
            onRoles(null);
            setRoles(null);
            setNotice("");
            setError("");
          }}
        >
          <Icon name="left" /> Schedules
        </Button>
      )}
      <header className="inspector-header">
        <h2>Schedules</h2>
      </header>
      {!chosen && (
        <div className="schedule-list-controls">
          <div className="schedule-list-toolbar">
            <Button
              className="button-primary"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                setError("");
                void defaults()
                  .then((d) => {
                    const targets = selected.flatMap((n) => {
                      const found = datasets.find(
                        (e) =>
                          identity(keyOf(e)) === n.identity &&
                          e.origin === "local" &&
                          !e.tombstone,
                      );
                      return found ? [keyOf(found)] : [];
                    });
                    setDraft({
                      definition: { ...d, build: { ...d.build, targets } },
                      record: null,
                    });
                  })
                  .catch((e) => setError(message(e)))
                  .finally(() => setBusy(false));
              }}
            >
              <Icon name="new" /> New schedule
            </Button>
            <Switch
              label="All workspace schedules"
              value={all}
              onChange={setAll}
            />
          </div>
          <input
            aria-label="Search schedules"
            placeholder="Search schedules…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
      )}
      {!chosen && (
        <>
          {selected.length > 0 && !all ? (
            <>
              {(
                [
                  ["targets", "Builds selected datasets"],
                  ["triggers", "Triggered by selected datasets"],
                ] as const
              ).map(([role, title]) => {
                const rows = matches.filter((row) => {
                  const definition = savedDefinition(row);
                  return definition && related(definition, scopeNodes)[role];
                });
                return rows.length ? (
                  <section className="schedule-list-group" key={role}>
                    <h3>{title}</h3>
                    {rows.map((row) => (
                      <ScheduleRow
                        key={row.id}
                        row={row}
                        onSelect={() => choose(row.id)}
                      />
                    ))}
                  </section>
                ) : null;
              })}
            </>
          ) : (
            matches.map((r) => (
              <ScheduleRow
                key={r.id}
                row={r}
                onSelect={() => {
                  choose(r.id);
                }}
              />
            ))
          )}
          {!matches.length && (
            <Empty>
              {selected.length && !all
                ? "No available schedules for selected nodes"
                : "No available schedules"}
            </Empty>
          )}
          {next && (
            <Button
              onClick={() => {
                const abort = new AbortController();
                void workspace.client
                  .read(
                    "ApiSchedulesV1",
                    "/api/v1/schedules",
                    { after: next },
                    abort.signal,
                  )
                  .then((r) => {
                    setRows([...rows, ...r.data.schedules]);
                    setNext(r.data.next_cursor);
                  })
                  .catch((e) => setError(message(e)));
              }}
            >
              Load more schedules
            </Button>
          )}
        </>
      )}
      {current && (
        <>
          <div className="schedule-detail-heading">
            <h3>{definition?.name ?? "Schedule needs review"}</h3>
            <Button
              className="schedule-view-button"
              aria-label="View schedule"
              title="View schedule"
              onClick={() => setViewOpen(true)}
            >
              <Icon name="external" />
              <span className="schedule-view-label">View schedule</span>
            </Button>
            {definition?.description && <p>{definition.description}</p>}
          </div>
          <p>
            <ScheduleState record={current} />
          </p>
          {!viewOpen && scheduleActions}
          {definition && (
            <>
              <ScheduleBuildScope plan={scopePlan} error={scopeError} />
              <h4>When to build</h4>
              <TriggerSummary
                trigger={definition.trigger}
                paths={path}
                compact
              />
            </>
          )}
          {hidden.length > 0 && (
            <Button
              disabled={!hidden.some((n) => n.paths.length)}
              onClick={() => onAdd(hidden.flatMap((n) => n.paths))}
            >
              Add hidden relevant datasets to view
            </Button>
          )}
          {definition && (
            <section className="schedule-branch-section">
              <h4>Branches</h4>
              <ScheduleBranches definition={definition} />
            </section>
          )}
          {definition && (
            <section className="schedule-options-section">
              <h4>Code and build options</h4>
              <ScheduleFacts definition={definition} />
            </section>
          )}
          {viewOpen && (
            <ScheduleModal
              key={current.id}
              workspace={workspace}
              state={state}
              record={current}
              definition={definition}
              path={path}
              actions={scheduleActions}
              scopePlan={scopePlan}
              scopeError={scopeError}
              onClose={() => setViewOpen(false)}
            />
          )}
          {deleting && (
            <DeleteDialog
              name={definition?.name ?? "this schedule"}
              busy={busy}
              onClose={() => setDeleting(false)}
              onDelete={() => {
                void action("delete");
              }}
            />
          )}
        </>
      )}
      {notice && (
        <p className="schedule-notice" role="status">
          <Icon name="info" />
          {notice}
        </p>
      )}
      {roleError && current && (
        <p role="status" className="muted">
          {roleError}
        </p>
      )}
      {error && (
        <p className="execution-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
function ScheduleRow({
  row,
  onSelect,
}: {
  row: ScheduleRecordV1;
  onSelect: () => void;
}) {
  const d = savedDefinition(row);
  return (
    <Button className="schedule-row" onClick={onSelect}>
      <Icon name="calendar" />
      <span>
        <strong>{d?.name ?? "Schedule needs review"}</strong>{" "}
        <small className="schedule-row-meta">
          <ScheduleState record={row} />{" "}
          <span>{d?.build.data_branch ?? "Branch unavailable"}</span>
        </small>
      </span>
      <Icon name="right" />
    </Button>
  );
}
