import { useEffect, useMemo, useRef, useState } from "react";
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
import { type Ready, Empty, Metric, Status, message } from "./execution-ui";
import {
  savedDefinition,
  related,
  identity,
  keyOf,
  minute,
  duration,
  ratio,
} from "./schedule-model";
import { ScheduleEditor, Switch } from "./ScheduleEditor";
import { historyDateRange, percent, position } from "./execution-model";
import { useBuildEvidence } from "./build-evidence";
import { BuildModal } from "./BuildReport";

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
}: {
  trigger: ScheduleDefinitionV1["trigger"];
  paths: (key: import("./generated/contracts").DatasetKey) => string;
}) {
  switch (trigger.kind) {
    case "manual":
      return <span>Manual only</span>;
    case "and":
    case "or":
      return (
        <div className="schedule-trigger-summary">
          <strong>
            {trigger.kind === "and" ? "All conditions" : "Any condition"}
          </strong>
          <ul>
            {trigger.children.map((c, i) => (
              <li key={i}>
                <TriggerSummary trigger={c} paths={paths} />
              </li>
            ))}
          </ul>
        </div>
      );
    case "cron":
      return (
        <span>
          {trigger.expression} · {trigger.timezone} ·{" "}
          {trigger.duplicate_time === "both"
            ? "both repeated times"
            : "first repeated time"}
        </span>
      );
    case "dataset_published":
    case "dataset_head_changed":
      return (
        <span>
          {paths(trigger.dataset)} ·{" "}
          {trigger.kind === "dataset_published" ? "published" : "head changed"}{" "}
          on {trigger.branch} ·{" "}
          {trigger.payload_mode === "pin" ? "exact version pin" : "signal only"}
          {trigger.include_resets ? " · includes resets" : ""}
        </span>
      );
    case "schedule_succeeded":
      return (
        <span>
          Schedule {trigger.schedule_id} succeeded
          {trigger.require_materialization ? " with materialization" : ""}
        </span>
      );
    case "build_succeeded":
      return (
        <span>
          Build of {trigger.targets.map(paths).join(", ")} succeeded on{" "}
          {trigger.branch}
        </span>
      );
  }
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
      <h3>Occurrence history</h3>
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
}: {
  workspace: Workspace;
  state: Ready;
  selected: readonly ApiLineageNodeV1[];
  visible: readonly string[];
  onRoles: (roles: ApiScheduleRolesV1 | null) => void;
  onPlan: (plan: PlanResultV1 | null) => void;
  onAdd: (paths: readonly string[]) => void;
  focusRequest?: { id: string; revision: number } | null;
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
  const [preview, setPreview] = useState<PlanResultV1 | null>(null);
  const [error, setError] = useState(""),
    [notice, setNotice] = useState(""),
    [busy, setBusy] = useState(false),
    [revision, setRevision] = useState(0),
    [deleting, setDeleting] = useState(false);
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
          setPreview(null);
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
  useEffect(() => {
    if (!current) {
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
          `/api/v1/schedules/${current.id}/roles`,
          {
            branch: context.branch,
            fallback: JSON.stringify(context.fallback_policy.slice(1)),
            ...(after ? { after } : {}),
          },
          abort.signal,
        );
        if (!result.context || visualContext(result.context) !== contextKey)
          throw new DOMException(
            "Schedule source context changed",
            "AbortError",
          );
        if (result.data.etag !== current.etag)
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
  }, [workspace, current, context, contextKey, onRoles]);
  const defaults = async () => {
    const abort = new AbortController();
    pending.current?.abort();
    pending.current = abort;
    return (
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
      if (!abort.signal.aborted) setRevision((n) => n + 1);
    } catch (e) {
      if (!abort.signal.aborted) setError(message(e));
    } finally {
      if (!abort.signal.aborted) setBusy(false);
    }
  };
  const relevant = new Map((roles?.nodes ?? []).map((n) => [n.identity, n]));
  for (const write of preview?.writes ?? []) {
    const id = `dataset:${preview?.workspace}:${write.dataset}`;
    if (!relevant.has(id))
      relevant.set(id, { identity: id, paths: [write.path], roles: [] });
  }
  for (const read of preview?.reads ?? []) {
    const id = `dataset:${read.origin_workspace ?? preview?.workspace}:${read.dataset}`;
    if (!relevant.has(id))
      relevant.set(id, {
        identity: id,
        paths: [read.path],
        roles: ["boundary"],
      });
  }
  const hidden = [...relevant.values()].filter(
    (n) => !visible.includes(n.identity),
  );
  const choose = (id: string) => {
    setChosen(id);
    setPreview(null);
    setRoles(null);
    onPlan(null);
    onRoles(null);
    setNotice("");
  };
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
          onDefaults={defaults}
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
          onCancel={() => setDraft(null)}
          onSave={async (d, signal) => {
            const saved = await workspace.client.saveSchedule(
              d,
              draft.record,
              signal,
            );
            if (signal.aborted) return;
            setRows((previous) => [
              saved,
              ...previous.filter((r) => r.id !== saved.id),
            ]);
            setChosen(saved.id);
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
      <div className="schedule-actions">
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
        <Switch label="All workspace schedules" value={all} onChange={setAll} />
      </div>
      <input
        aria-label="Search schedules"
        placeholder="Search schedules…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      {!chosen && (
        <>
          <p className="muted">
            {all
              ? "All saved workspace schedules"
              : selected.length
                ? "Schedules related to selected datasets"
                : "Schedules related to visible graph datasets"}
          </p>
          {selected.length > 0 && !all ? (
            <>
              {(
                [
                  ["targets", "Builds selected datasets"],
                  ["triggers", "Triggered by selected datasets"],
                ] as const
              ).map(([role, title]) => (
                <section key={role}>
                  <h3>{title}</h3>
                  {matches
                    .filter((r) => {
                      const d = savedDefinition(r);
                      return d && related(d, scopeNodes)[role];
                    })
                    .map((r) => (
                      <ScheduleRow
                        key={r.id}
                        row={r}
                        onSelect={() => {
                          choose(r.id);
                        }}
                      />
                    ))}
                </section>
              ))}
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
              No schedules match this graph context. Create a schedule or show
              all workspace schedules.
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
          <Button
            onClick={() => {
              setChosen(null);
              setPreview(null);
              onPlan(null);
              onRoles(null);
              setRoles(null);
            }}
          >
            <Icon name="left" /> Schedules
          </Button>
          <h3>{definition?.name ?? "Schedule needs review"}</h3>
          <p className="muted">
            {current.paused ? "Paused" : "Active"}
            {current.needs_review ? " · Review required" : ""}
          </p>
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
          {definition && (
            <>
              <p>{definition.description}</p>
              <dl className="schedule-details">
                <dt>Data branch</dt>
                <dd>{definition.build.data_branch}</dd>
                <dt>Fallback order</dt>
                <dd>
                  {[
                    definition.build.data_branch,
                    ...definition.build.fallback_branches,
                  ].join(" → ")}
                </dd>
                <dt>Source</dt>
                <dd>
                  {definition.build.source.kind === "git_ref"
                    ? definition.build.source.ref
                    : definition.build.source.kind === "fixed_snapshot"
                      ? `Fixed snapshot ${definition.build.source.snapshot_id}`
                      : "Mutable working tree"}
                </dd>
                <dt>Scope</dt>
                <dd>{definition.build.build_mode}</dd>
                <dt>Attempts / retry classes</dt>
                <dd>
                  {definition.policies.max_attempts} /{" "}
                  {definition.policies.retryable_classes.join(", ") || "none"}
                </dd>
                <dt>Overlap</dt>
                <dd>
                  {definition.policies.allow_overlapping_builds
                    ? "Allowed with write reservations"
                    : definition.policies.overlap_policy.replaceAll("_", " ")}
                </dd>
                <dt>Missed ticks</dt>
                <dd>
                  {definition.policies.misfire_policy.replaceAll("_", " ")}
                </dd>
                <dt>Event expiry</dt>
                <dd>
                  {definition.policies.token_window_seconds === null
                    ? "No expiry acknowledged"
                    : `${definition.policies.token_window_seconds} s`}
                </dd>
                <dt>Automatic burst limit</dt>
                <dd>
                  {definition.policies.max_consecutive_builds} ·{" "}
                  {definition.policies.minimum_delay_seconds} s minimum delay
                </dd>
              </dl>
              <h4>Datasets to build</h4>
              <ul>
                {definition.build.targets.map((k) => (
                  <li key={identity(k)}>{path(k)}</li>
                ))}
              </ul>
              <h4>When to build</h4>
              <TriggerSummary trigger={definition.trigger} paths={path} />
              <details>
                <summary>Input policies and boundaries</summary>
                <h4>Read boundaries</h4>
                <ul>
                  {definition.build.boundaries.map((k) => (
                    <li key={identity(k)}>{path(k)}</li>
                  ))}
                </ul>
                <pre>
                  {JSON.stringify(
                    {
                      input_fallback_policy:
                        definition.build.input_fallback_policy,
                      provider_fallback_policies:
                        definition.build.provider_fallback_policies,
                      exclusions: definition.build.exclusions,
                      force: definition.build.force,
                      abort_on_failure: definition.policies.abort_on_failure,
                    },
                    null,
                    2,
                  )}
                </pre>
              </details>
              <div className="schedule-actions">
                <Button
                  disabled={busy}
                  onClick={() => {
                    const abort = new AbortController();
                    pending.current?.abort();
                    pending.current = abort;
                    setBusy(true);
                    void workspace.client
                      .schedulePreview(definition, context, abort.signal)
                      .then((p) => {
                        if (!abort.signal.aborted) {
                          onPlan(p);
                          setPreview(p);
                          setNotice(
                            `Complete scope: ${p.writes.length} jobs · ${p.targets.length} ${p.targets.length === 1 ? "target" : "targets"}. Cached inputs remain boundaries.`,
                          );
                        }
                      })
                      .catch((e) => {
                        if (!abort.signal.aborted) setError(message(e));
                      })
                      .finally(() => {
                        if (!abort.signal.aborted) setBusy(false);
                      });
                  }}
                >
                  Preview build scope
                </Button>
              </div>
            </>
          )}
          {roles && (
            <p className="muted">
              Saved roles: {relevant.size} relevant datasets ·{" "}
              {
                [...relevant.values()].filter((n) =>
                  visible.includes(n.identity),
                ).length
              }{" "}
              visible · {hidden.length} hidden
            </p>
          )}
          {hidden.length > 0 && (
            <Button
              disabled={!hidden.some((n) => n.paths.length)}
              onClick={() => onAdd(hidden.flatMap((n) => n.paths))}
            >
              Add hidden relevant datasets to view
            </Button>
          )}
          <ScheduleHistory
            key={current.id}
            workspace={workspace}
            state={state}
            record={current}
          />
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
        <strong>{d?.name ?? "Schedule needs review"}</strong>
        <small>
          {row.paused ? "Paused" : "Active"} ·{" "}
          {d?.build.data_branch ?? "Branch unavailable"}
          {row.needs_review ? " · review required" : ""}
        </small>
      </span>
      <Icon name="right" />
    </Button>
  );
}
