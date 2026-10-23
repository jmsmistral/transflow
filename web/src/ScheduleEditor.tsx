import {
  useEffect,
  useState,
  useRef,
  useCallback,
  useId,
  type ReactNode,
} from "react";
import type {
  ApiDatasetV1,
  ScheduleDefinitionV1,
  ScheduleTriggerV1,
  DatasetKey,
  ScheduleRecordV1,
  SchedulePoliciesV1,
  ApiScheduleClockPreviewV1,
  ApiLineageNodeV1,
} from "./generated/contracts";
import { Button } from "./components";
import { Icon } from "./Icons";
import { decode } from "./api/client";
import type { Workspace } from "./workspace";
import {
  identity,
  keyOf,
  savedDefinition,
  graphKey,
  editGraphRoles,
  connectionIssue,
  usesTriggerConnections,
  withTriggerConnections,
  type ScheduleEditSession,
  type ScheduleGraphAction,
} from "./schedule-model";
import { ScheduleDisclosure } from "./ScheduleDisclosure";
import { ScheduleBranchesEditor } from "./ScheduleBranchesEditor";

function hasClock(trigger: ScheduleTriggerV1): boolean {
  return (
    trigger.kind === "cron" ||
    ((trigger.kind === "and" || trigger.kind === "or") &&
      trigger.children.some(hasClock))
  );
}
function clockLabel(trigger: ScheduleTriggerV1, id: string): string {
  if (trigger.kind === "cron" && trigger.id === id)
    return `${trigger.expression} · ${trigger.timezone}`;
  if (trigger.kind === "and" || trigger.kind === "or") {
    for (const child of trigger.children) {
      const label = clockLabel(child, id);
      if (label) return label;
    }
  }
  return "";
}
export function Switch({
  label,
  value,
  onChange,
  disabled = false,
}: {
  label: string;
  value: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className="schedule-switch">
      <button
        type="button"
        role="switch"
        aria-checked={value}
        aria-label={label}
        disabled={disabled}
        onClick={() => onChange(!value)}
      >
        <span />
      </button>
      <span>{label}</span>
    </label>
  );
}
function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="schedule-field">
      <span>{label}</span>
      {children}
    </label>
  );
}
function DurationField({
  label,
  value,
  onChange,
  allowZero = false,
}: {
  label: string;
  value: number;
  onChange: (seconds: number) => void;
  allowZero?: boolean;
}) {
  const [unit, setUnit] = useState(
    value > 0 && value % 3600 === 0
      ? 3600
      : value > 0 && value % 60 === 0
        ? 60
        : 1,
  );
  const id = useId();
  return (
    <div className="schedule-duration-field">
      <label htmlFor={id}>{label}</label>
      <div>
        <input
          id={id}
          type="number"
          required
          min={allowZero ? 0 : 1 / unit}
          step="any"
          value={Number.isFinite(value) ? value / unit : ""}
          onChange={(e) =>
            onChange(
              Number.isFinite(e.target.valueAsNumber)
                ? Math.round(e.target.valueAsNumber * unit)
                : NaN,
            )
          }
        />
        <select
          aria-label={`${label} unit`}
          value={unit}
          onChange={(e) => setUnit(Number(e.target.value))}
        >
          <option value={1}>seconds</option>
          <option value={60}>minutes</option>
          <option value={3600}>hours</option>
        </select>
      </div>
      {allowZero && <small>Set to 0 to disable this limit.</small>}
    </div>
  );
}
export function DatasetChoices({
  label,
  value,
  datasets,
  onChange,
  local = false,
}: {
  label: string;
  value: readonly DatasetKey[];
  datasets: readonly ApiDatasetV1[];
  onChange: (v: readonly DatasetKey[]) => void;
  local?: boolean;
}) {
  const [search, setSearch] = useState("");
  const available = datasets.filter(
    (d) =>
      !d.tombstone &&
      (!local || d.origin === "local") &&
      d.path.toLowerCase().includes(search.toLowerCase()),
  );
  const missing = value.filter(
    (k) => !datasets.some((d) => identity(keyOf(d)) === identity(k)),
  );
  return (
    <fieldset className="schedule-datasets">
      <legend>{label}</legend>
      <input
        aria-label={`Search ${label.toLowerCase()}`}
        placeholder="Search datasets…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
      />
      <div className="schedule-choices">
        {available.map((d) => {
          const key = keyOf(d),
            selected = value.some((k) => identity(k) === identity(key));
          return (
            <label key={identity(key)}>
              <input
                type="checkbox"
                checked={selected}
                onChange={() =>
                  onChange(
                    selected
                      ? value.filter((k) => identity(k) !== identity(key))
                      : [...value, key],
                  )
                }
              />
              <span>
                {d.path}
                {d.origin === "external" && <small>External input</small>}
              </span>
            </label>
          );
        })}
        {missing.map((k) => (
          <label key={identity(k)}>
            <input
              type="checkbox"
              checked
              onChange={() =>
                onChange(value.filter((v) => identity(v) !== identity(k)))
              }
            />
            <span>Unresolved dataset {k.dataset_id}</span>
          </label>
        ))}
        {!available.length && (
          <p className="muted">No matching registered datasets.</p>
        )}
      </div>
    </fieldset>
  );
}
function newTrigger(
  kind: string,
  datasets: readonly ApiDatasetV1[],
  branch: string,
): ScheduleTriggerV1 {
  const id = `condition-${crypto.randomUUID().slice(0, 8)}`;
  const first = datasets[0];
  switch (kind) {
    case "and":
    case "or":
      return {
        kind,
        children: [
          newTrigger("cron", datasets, branch),
          newTrigger("cron", datasets, branch),
        ],
      };
    case "cron":
      return {
        kind,
        id,
        expression: "0 9 * * 1-5",
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
        duplicate_time: "earliest",
      };
    case "dataset_published":
    case "dataset_head_changed":
      return first
        ? {
            kind,
            id,
            dataset: keyOf(first),
            branch,
            payload_mode: "pin",
            include_resets: false,
          }
        : { kind: "manual" };
    case "build_succeeded":
      return { kind, id, targets: first ? [keyOf(first)] : [], branch };
    case "schedule_succeeded":
      return { kind, id, schedule_id: "", require_materialization: true };
    default:
      return { kind: "manual" };
  }
}
const kinds = [
  ["manual", "Manual only"],
  ["cron", "Time (cron)"],
  ["dataset_published", "Dataset published"],
  ["dataset_head_changed", "Dataset head changed"],
  ["build_succeeded", "Build succeeded"],
  ["schedule_succeeded", "Schedule succeeded"],
  ["and", "All conditions (AND)"],
  ["or", "Any condition (OR)"],
] as const;
export function TriggerEditor({
  value,
  onChange,
  datasets,
  schedules,
  branch,
  depth = 0,
  paths,
}: {
  value: ScheduleTriggerV1;
  onChange: (v: ScheduleTriggerV1) => void;
  datasets: readonly ApiDatasetV1[];
  schedules: readonly ScheduleRecordV1[];
  branch: string;
  depth?: number;
  paths?: ReadonlyMap<string, string>;
}) {
  const group = value.kind === "and" || value.kind === "or";
  const label = kinds.find(([kind]) => kind === value.kind)?.[1] ?? value.kind;
  const upstream =
    value.kind === "schedule_succeeded"
      ? schedules.find((s) => s.id === value.schedule_id)
      : undefined;
  const subject =
    "dataset" in value
      ? (datasets.find((d) => identity(keyOf(d)) === identity(value.dataset))
          ?.path ??
        paths?.get(identity(value.dataset)) ??
        "Choose a dataset")
      : value.kind === "cron"
        ? `${value.expression} · ${value.timezone}`
        : value.kind === "schedule_succeeded"
          ? ((upstream ? savedDefinition(upstream)?.name : undefined) ??
            "Choose a schedule")
          : value.kind === "build_succeeded"
            ? `${value.targets.length} build targets`
            : value.kind === "manual"
              ? "Run this schedule when you choose."
              : `${value.children.length} conditions`;
  return (
    <ScheduleDisclosure
      title={label}
      description={subject}
      initialOpen={depth === 0 || group}
      className="schedule-trigger-card"
    >
      {group ? (
        <div className="schedule-match">
          <span>Match</span>
          <div role="group" aria-label="Combine conditions">
            <Button
              aria-pressed={value.kind === "and"}
              onClick={() => onChange({ ...value, kind: "and" })}
            >
              All (AND)
            </Button>
            <Button
              aria-pressed={value.kind === "or"}
              onClick={() => onChange({ ...value, kind: "or" })}
            >
              Any (OR)
            </Button>
          </div>
          <p className="muted">
            {value.kind === "and"
              ? "Build after every condition has occurred."
              : "Build when any condition occurs. Separate matches may start separate builds."}
          </p>
        </div>
      ) : (
        <Field label="Condition type">
          <select
            value={value.kind}
            onChange={(e) => {
              const kind = e.target.value;
              if (kind === "and" || kind === "or") {
                onChange({
                  kind,
                  children:
                    value.kind === "manual"
                      ? [
                          newTrigger("cron", datasets, branch),
                          newTrigger("cron", datasets, branch),
                        ]
                      : [value, newTrigger("cron", datasets, branch)],
                });
              } else onChange(newTrigger(kind, datasets, branch));
            }}
          >
            {kinds
              .filter(
                ([kind]) =>
                  (depth === 0 || kind !== "manual") &&
                  (depth < 7 || !["and", "or"].includes(kind)),
              )
              .map(([kind, name]) => (
                <option key={kind} value={kind}>
                  {name}
                </option>
              ))}
          </select>
        </Field>
      )}
      {value.kind === "cron" && (
        <>
          <Field label="Cron expression">
            <input
              placeholder="0 9 * * 1-5"
              value={value.expression}
              onChange={(e) =>
                onChange({ ...value, expression: e.target.value })
              }
            />
          </Field>
          <Field label="Timezone">
            <input
              value={value.timezone}
              onChange={(e) => onChange({ ...value, timezone: e.target.value })}
            />
          </Field>
          <Field label="Repeated local time">
            <select
              value={value.duplicate_time}
              onChange={(e) =>
                onChange({
                  ...value,
                  duplicate_time:
                    e.target.value === "both" ? "both" : "earliest",
                })
              }
            >
              <option value="earliest">First occurrence</option>
              <option value="both">Both occurrences</option>
            </select>
          </Field>
        </>
      )}
      {(value.kind === "dataset_published" ||
        value.kind === "dataset_head_changed") && (
        <>
          <Field label="Dataset">
            <select
              value={identity(value.dataset)}
              onChange={(e) => {
                const d = datasets.find(
                  (d) => identity(keyOf(d)) === e.target.value,
                );
                if (d) onChange({ ...value, dataset: keyOf(d) });
              }}
            >
              {!datasets.some(
                (d) => identity(keyOf(d)) === identity(value.dataset),
              ) && (
                <option value={identity(value.dataset)}>
                  {paths?.get(identity(value.dataset)) ?? "Unresolved dataset"}
                </option>
              )}
              {datasets
                .filter((d) => !d.tombstone)
                .map((d) => (
                  <option key={identity(keyOf(d))} value={identity(keyOf(d))}>
                    {d.path}
                  </option>
                ))}
            </select>
          </Field>
          <Field label="Event branch">
            <input
              value={value.branch}
              onChange={(e) => onChange({ ...value, branch: e.target.value })}
            />
          </Field>
          {value.kind === "dataset_head_changed" && (
            <Switch
              label="Include resets"
              value={value.include_resets}
              onChange={(include_resets) =>
                onChange({ ...value, include_resets })
              }
            />
          )}
        </>
      )}
      {value.kind === "schedule_succeeded" && (
        <>
          <Field label="Upstream schedule">
            <select
              value={value.schedule_id}
              onChange={(e) =>
                onChange({ ...value, schedule_id: e.target.value })
              }
            >
              <option value="">Select a schedule…</option>
              {schedules.map((s) => (
                <option key={s.id} value={s.id}>
                  {savedDefinition(s)?.name ?? s.id}
                </option>
              ))}
            </select>
          </Field>
          <Switch
            label="Require new materialization"
            value={value.require_materialization}
            onChange={(require_materialization) =>
              onChange({ ...value, require_materialization })
            }
          />
        </>
      )}
      {value.kind === "build_succeeded" && (
        <>
          <DatasetChoices
            label="Build targets"
            value={value.targets}
            datasets={datasets}
            onChange={(targets) => onChange({ ...value, targets })}
          />
          <Field label="Event branch">
            <input
              value={value.branch}
              onChange={(e) => onChange({ ...value, branch: e.target.value })}
            />
          </Field>
        </>
      )}
      {(value.kind === "and" || value.kind === "or") && (
        <>
          {value.children.map((child, i) => (
            <div
              className="schedule-child"
              key={"id" in child ? child.id : `group-${i}`}
            >
              {i > 0 && (
                <div className="schedule-condition-join">
                  {value.kind.toUpperCase()}
                </div>
              )}
              <TriggerEditor
                value={child}
                onChange={(v) =>
                  onChange({
                    ...value,
                    children: value.children.map((c, j) => (j === i ? v : c)),
                  })
                }
                datasets={datasets}
                schedules={schedules}
                branch={branch}
                depth={depth + 1}
                {...(paths ? { paths } : {})}
              />
              <Button
                className="schedule-remove-condition"
                aria-label={`Remove condition ${i + 1}`}
                onClick={() => {
                  const children = value.children.filter((_, j) => j !== i);
                  const remaining = children[0];
                  if (children.length === 1 && remaining) onChange(remaining);
                  else if (children.length > 1)
                    onChange({ ...value, children });
                }}
              >
                Remove
              </Button>
            </div>
          ))}
          <Button
            className="schedule-add-button"
            disabled={value.children.length >= 64}
            onClick={() =>
              onChange({
                ...value,
                children: [
                  ...value.children,
                  newTrigger("cron", datasets, branch),
                ],
              })
            }
          >
            Add condition
          </Button>
        </>
      )}
      {!group && value.kind !== "manual" && depth < 7 && (
        <Button
          className="schedule-add-button"
          onClick={() =>
            onChange({
              kind: "and",
              children: [value, newTrigger("cron", datasets, branch)],
            })
          }
        >
          Add condition
        </Button>
      )}
    </ScheduleDisclosure>
  );
}
function PolicyJson({
  label,
  value,
  onChange,
}: {
  label: string;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const [text, setText] = useState(() => JSON.stringify(value, null, 2));
  return (
    <Field label={label}>
      <textarea
        value={text}
        rows={4}
        onChange={(e) => {
          setText(e.target.value);
          try {
            onChange(JSON.parse(e.target.value) as unknown);
            e.target.setCustomValidity("");
          } catch {
            e.target.setCustomValidity("Enter valid JSON");
          }
        }}
      />
    </Field>
  );
}
export function ScheduleEditor({
  workspace,
  initial,
  record,
  datasets,
  schedules,
  onSave,
  onCancel,
  selected = [],
  onEdit,
  onReload,
}: {
  workspace: Workspace;
  initial: ScheduleDefinitionV1;
  record: ScheduleRecordV1 | null;
  datasets: readonly ApiDatasetV1[];
  schedules: readonly ScheduleRecordV1[];
  onSave: (
    d: ScheduleDefinitionV1,
    signal: AbortSignal,
    exitToList?: boolean,
  ) => Promise<void>;
  onCancel: () => void;
  selected?: readonly ApiLineageNodeV1[];
  onEdit?: (session: ScheduleEditSession | null) => void;
  onReload?: (signal: AbortSignal) => Promise<void>;
}) {
  const [draft, setDraft] = useState(() =>
      withTriggerConnections(initial, usesTriggerConnections(initial)),
    ),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [clock, setClock] = useState<ApiScheduleClockPreviewV1 | null>(null),
    [clockError, setClockError] = useState("");
  const [graphPaths, setGraphPaths] = useState<ReadonlyMap<string, string>>(
    new Map(),
  );
  const [connecting, setConnecting] = useState(() =>
    usesTriggerConnections(initial),
  );
  const connectionError = connecting ? connectionIssue(draft) : "";
  const saveAndExit = useRef(false);
  const [policyError, setPolicyError] = useState("");
  const [confirmExit, setConfirmExit] = useState(false);
  const [touched, setTouched] = useState(false);
  const exitDialog = useRef<HTMLDialogElement>(null),
    form = useRef<HTMLFormElement>(null),
    cancelButton = useRef<HTMLButtonElement>(null);
  const exitTitle = useId();
  const act = useCallback(
    (action: ScheduleGraphAction, nodes: readonly ApiLineageNodeV1[]) => {
      setGraphPaths(
        (old) =>
          new Map([
            ...old,
            ...nodes.map(
              (n) => [n.identity, n.paths[0] ?? n.identity] as const,
            ),
          ]),
      );
      setDraft((d) => {
        const next = editGraphRoles(d, action, nodes);
        return connecting ? withTriggerConnections(next, true) : next;
      });
    },
    [connecting],
  );
  const exit = useCallback(() => cancelButton.current?.click(), []);
  useEffect(() => {
    onEdit?.({ definition: draft, busy, act, exit });
  }, [onEdit, draft, busy, act, exit]);
  useEffect(() => () => onEdit?.(null), [onEdit]);
  useEffect(() => {
    if (confirmExit) exitDialog.current?.showModal();
    else exitDialog.current?.close();
  }, [confirmExit]);
  const requestExit = () => {
    if (
      JSON.stringify(draft) !== JSON.stringify(initial) ||
      policyError ||
      (touched && form.current?.querySelector(":invalid"))
    )
      setConfirmExit(true);
    else onCancel();
  };
  const additions = selected.filter((n) => {
    const key = graphKey(n);
    return (
      key &&
      !n.external &&
      n.producer &&
      !draft.build.targets.some((k) => identity(k) === identity(key))
    );
  });
  const build = (patch: Partial<ScheduleDefinitionV1["build"]>) =>
    setDraft((d) => {
      const next = { ...d, build: { ...d.build, ...patch } };
      return connecting ? withTriggerConnections(next, true) : next;
    });
  const policies = (patch: Partial<SchedulePoliciesV1>) =>
    setDraft((d) => ({ ...d, policies: { ...d.policies, ...patch } }));
  useEffect(() => {
    const abort = new AbortController();
    const timer = setTimeout(() => {
      // Incomplete graph drafts and event/manual-only definitions have no clock
      // preview to request. Do not show whole-definition validation as a clock error.
      if (
        !draft.name.trim() ||
        !draft.build.targets.length ||
        !hasClock(draft.trigger) ||
        connectionError
      ) {
        setClock(null);
        setClockError("");
        return;
      }
      void workspace.client
        .scheduleClock(draft, abort.signal)
        .then((v) => {
          if (!abort.signal.aborted) {
            setClock(v);
            setClockError("");
          }
        })
        .catch((e) => {
          if (!abort.signal.aborted) {
            setClock(null);
            setClockError(
              e instanceof Error ? e.message : "Clock preview failed",
            );
          }
        });
    }, 400);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [draft, workspace, connectionError]);
  const saveRequest = useRef<AbortController | null>(null);
  useEffect(() => () => saveRequest.current?.abort(), []);
  return (
    <form
      ref={form}
      className="schedule-editor"
      onChangeCapture={() => setTouched(true)}
      onSubmit={(e) => {
        e.preventDefault();
        if (policyError || connectionError) {
          setError(policyError || connectionError);
          return;
        }
        setBusy(true);
        setError("");
        const abort = new AbortController();
        saveRequest.current?.abort();
        saveRequest.current = abort;
        void Promise.resolve()
          .then(() =>
            onSave(
              decode("ScheduleDefinitionV1", draft),
              abort.signal,
              saveAndExit.current,
            ),
          )
          .catch((e) => {
            if (!abort.signal.aborted)
              setError(e instanceof Error ? e.message : "Schedule save failed");
          })
          .finally(() => {
            if (!abort.signal.aborted) {
              setBusy(false);
              saveAndExit.current = false;
            }
          });
      }}
    >
      <Button
        className="schedule-back schedule-back-top"
        disabled={busy}
        onClick={requestExit}
      >
        <Icon name="left" /> Schedules
      </Button>
      <header className="schedule-editor-header">
        <h3>{record ? "Edit schedule" : "Create schedule"}</h3>
        <div className="schedule-editor-actions">
          <Button
            type="submit"
            onClick={() => {
              saveAndExit.current = false;
            }}
            className="button-primary"
            disabled={
              busy ||
              !!policyError ||
              !!connectionError ||
              !draft.build.targets.length
            }
          >
            {busy ? "Saving…" : "Save schedule"}
          </Button>
          <button
            type="button"
            className="button"
            ref={cancelButton}
            disabled={busy}
            onClick={requestExit}
          >
            Cancel
          </button>
        </div>
      </header>
      <div className="schedule-identity-fields">
        <input
          aria-label="Schedule name"
          placeholder="Schedule name"
          required
          value={draft.name}
          onChange={(e) => setDraft({ ...draft, name: e.target.value })}
        />
        <textarea
          aria-label="Description"
          placeholder="Description"
          rows={2}
          value={draft.description}
          onChange={(e) => setDraft({ ...draft, description: e.target.value })}
        />
      </div>
      <section
        className="schedule-section schedule-targets"
        aria-label="Datasets to build"
      >
        <h4>Datasets to build</h4>
        <div className="schedule-target-list" role="list">
          {draft.build.targets.map((k) => (
            <div className="schedule-dataset" role="listitem" key={identity(k)}>
              <Icon name="table" />
              <span>
                {datasets.find((d) => identity(keyOf(d)) === identity(k))
                  ?.path ??
                  graphPaths.get(identity(k)) ??
                  selected.find((n) => n.identity === identity(k))?.paths[0] ??
                  k.dataset_id}
              </span>
              <Button
                aria-label={`Remove target ${datasets.find((d) => identity(keyOf(d)) === identity(k))?.path ?? k.dataset_id}`}
                onClick={() =>
                  build({
                    targets: draft.build.targets.filter(
                      (v) => identity(v) !== identity(k),
                    ),
                  })
                }
              >
                <Icon name="remove" />
              </Button>
            </div>
          ))}
          {!draft.build.targets.length && (
            <p className="muted">
              Select producer datasets on the graph to add targets.
            </p>
          )}
        </div>
        <Button
          className="schedule-add-button"
          disabled={busy || !additions.length}
          onClick={() => act("target", additions)}
        >
          Add {additions.length} selected{" "}
          {additions.length === 1 ? "node" : "nodes"}
        </Button>
      </section>
      <section
        className="schedule-section schedule-trigger-section"
        aria-label="When to build"
      >
        <h4>When to build</h4>
        <TriggerEditor
          value={draft.trigger}
          onChange={(trigger) =>
            setDraft((d) => {
              const next = { ...d, trigger };
              return connecting ? withTriggerConnections(next, true) : next;
            })
          }
          datasets={datasets}
          schedules={schedules}
          branch={draft.build.data_branch}
          paths={graphPaths}
        />
        {clock && clock.leaves.length > 0 && (
          <ScheduleDisclosure
            title="Next intended times"
            description="Preview upcoming times in each condition’s timezone"
            className="schedule-clock"
          >
            {clock.leaves.map((l) => (
              <div key={l.id}>
                <span>{clockLabel(draft.trigger, l.id)}</span>
                <ul>
                  {l.fires.map((f) => (
                    <li key={f.at_us}>
                      {f.local.replace("T", " ")} · {f.timezone}
                    </li>
                  ))}
                </ul>
                <small>Timezone rules {l.fires[0]?.tzdb_version}</small>
              </div>
            ))}
          </ScheduleDisclosure>
        )}
        {clockError && (
          <p role="status" className="muted">
            {clockError}
          </p>
        )}
      </section>
      <section
        className="schedule-connecting"
        aria-label="Connecting build settings"
      >
        <Switch
          label="Connecting build"
          value={connecting}
          disabled={busy || (!connecting && !!connectionIssue(draft))}
          onChange={(enabled) => {
            setConnecting(enabled);
            setDraft((d) => withTriggerConnections(d, enabled));
          }}
        />
        <p className="muted" role={connectionError ? "alert" : undefined}>
          {connecting
            ? connectionIssue(draft) ||
              "Attempt to build every dataset between Triggers and Targets."
            : "Build only selected targets, using published data for other inputs."}
        </p>
        {!connecting && connectionIssue(draft) && (
          <p className="muted">{connectionIssue(draft)}</p>
        )}
        {initial.build.build_mode !== "selected" &&
          !usesTriggerConnections(initial) && (
            <p className="muted" role="status">
              This schedule used an advanced build scope. Saving applies the
              Connecting build choice above; with it off, only targets are
              built.
            </p>
          )}
      </section>
      <ScheduleBranchesEditor
        workspace={workspace}
        branch={draft.build.data_branch}
        fallback={draft.build.fallback_branches}
        onBranch={(data_branch) => build({ data_branch })}
        onFallback={(fallback_branches) => build({ fallback_branches })}
      />
      <ScheduleDisclosure
        title="Advanced options"
        description="Build behavior, retries and timing"
        className="schedule-advanced"
      >
        <ScheduleDisclosure
          title="Build reuse"
          description="Choose whether to reuse up-to-date results"
        >
          <Switch
            label="Force build on up-to-date datasets"
            value={draft.build.force}
            onChange={(force) => build({ force })}
          />
        </ScheduleDisclosure>
        <ScheduleDisclosure
          title="Failures and retries"
          description="Control what happens when a job fails"
        >
          <Switch
            label="Abort dependent work after a required failure"
            value={draft.policies.abort_on_failure}
            onChange={(abort_on_failure) => policies({ abort_on_failure })}
          />
          <Field label="Maximum attempts per job">
            <input
              type="number"
              min={1}
              max={10}
              value={draft.policies.max_attempts}
              onChange={(e) =>
                policies({ max_attempts: e.target.valueAsNumber })
              }
            />
          </Field>
          {draft.policies.max_attempts > 1 && (
            <div className="schedule-policy-group">
              <p className="schedule-control-label">Retry these failures</p>
              {(
                [
                  "worker_crash",
                  "provider_unavailable",
                  "resource_unavailable",
                ] as const
              ).map((c) => (
                <label className="schedule-check" key={c}>
                  <input
                    type="checkbox"
                    checked={draft.policies.retryable_classes.includes(c)}
                    onChange={(e) =>
                      policies({
                        retryable_classes: e.target.checked
                          ? [...draft.policies.retryable_classes, c]
                          : draft.policies.retryable_classes.filter(
                              (v) => v !== c,
                            ),
                      })
                    }
                  />
                  {c.replaceAll("_", " ")}
                </label>
              ))}
              <small>
                Authentication, protocol, input integrity and data-quality
                failures are never retried by these options.
              </small>
            </div>
          )}
        </ScheduleDisclosure>
        <ScheduleDisclosure
          title="Overlapping runs"
          description="Choose what happens while a build is active"
        >
          <Switch
            label="Allow overlapping builds"
            value={draft.policies.allow_overlapping_builds}
            onChange={(allow_overlapping_builds) =>
              policies({ allow_overlapping_builds })
            }
          />
          {draft.policies.allow_overlapping_builds && (
            <p className="schedule-notice">
              <Icon name="info" /> Concurrent builds still wait for exclusive
              dataset write reservations.
            </p>
          )}
          <Field label="When a build is already active">
            <select
              value={draft.policies.overlap_policy}
              onChange={(e) =>
                policies({
                  overlap_policy:
                    e.target.value === "skip"
                      ? "skip"
                      : e.target.value === "queue"
                        ? "queue"
                        : "coalesce_latest",
                })
              }
            >
              <option value="coalesce_latest">
                Keep the latest compatible pending occurrence
              </option>
              <option value="queue">Queue pending occurrences</option>
              <option value="skip">Skip with a recorded reason</option>
            </select>
          </Field>
          {draft.policies.overlap_policy !== "skip" && (
            <Field label="Maximum pending occurrences">
              <input
                type="number"
                min={1}
                max={100}
                value={draft.policies.max_pending}
                onChange={(e) =>
                  policies({ max_pending: e.target.valueAsNumber })
                }
              />
            </Field>
          )}
        </ScheduleDisclosure>
        <ScheduleDisclosure
          title="Timing and limits"
          description="Missed runs, event expiry and execution limits"
        >
          <Field label="Missed clock ticks">
            <select
              value={draft.policies.misfire_policy}
              onChange={(e) =>
                policies({
                  misfire_policy:
                    e.target.value === "skip"
                      ? "skip"
                      : e.target.value === "catch_up"
                        ? "catch_up"
                        : "coalesce_latest",
                })
              }
            >
              <option value="skip">Skip</option>
              <option value="coalesce_latest">
                Keep latest per time condition
              </option>
              <option value="catch_up">Bounded catch-up</option>
            </select>
          </Field>
          {draft.policies.misfire_policy === "catch_up" && (
            <Field label="Maximum catch-up ticks">
              <input
                type="number"
                min={1}
                max={100}
                value={draft.policies.max_catch_up}
                onChange={(e) =>
                  policies({ max_catch_up: e.target.valueAsNumber })
                }
              />
            </Field>
          )}
          <Switch
            label="Keep events until they are used"
            value={draft.policies.acknowledge_no_expiry}
            onChange={(v) =>
              policies({
                acknowledge_no_expiry: v,
                token_window_seconds: v ? null : 86400,
              })
            }
          />
          {draft.policies.acknowledge_no_expiry && (
            <p className="muted">
              Events will not expire. Older events may combine with new ones
              much later.
            </p>
          )}
          {draft.policies.token_window_seconds !== null && (
            <DurationField
              label="Event expiry"
              value={draft.policies.token_window_seconds}
              onChange={(seconds) =>
                policies({ token_window_seconds: seconds })
              }
            />
          )}
          <Field label="Maximum consecutive automatic builds">
            <input
              type="number"
              min={1}
              max={100}
              value={draft.policies.max_consecutive_builds}
              onChange={(e) =>
                policies({ max_consecutive_builds: e.target.valueAsNumber })
              }
            />
          </Field>
          <DurationField
            label="Minimum delay between automatic builds"
            value={draft.policies.minimum_delay_seconds}
            onChange={(seconds) => policies({ minimum_delay_seconds: seconds })}
            allowZero
          />
          <DurationField
            label="Job time limit"
            value={draft.build.timeout_seconds}
            onChange={(seconds) => build({ timeout_seconds: seconds })}
            allowZero
          />
          <DurationField
            label="Validation time limit"
            value={draft.build.validation_timeout_seconds}
            onChange={(seconds) =>
              build({ validation_timeout_seconds: seconds })
            }
            allowZero
          />
        </ScheduleDisclosure>
        <ScheduleDisclosure
          title="Input datasets"
          description="Exclusions and refreshed sources"
        >
          <DatasetChoices
            label="Excluded datasets"
            value={draft.build.exclusions}
            datasets={datasets}
            local
            onChange={(exclusions) => build({ exclusions })}
          />
          <DatasetChoices
            label="Refresh source datasets"
            value={draft.build.refresh_sources}
            datasets={datasets}
            local
            onChange={(refresh_sources) => build({ refresh_sources })}
          />
          <Switch
            label="Require current boundary data"
            value={draft.build.require_current}
            onChange={(require_current) => build({ require_current })}
          />
        </ScheduleDisclosure>
        <ScheduleDisclosure
          title="Input policies and parameters"
          description="Overrides for named inputs and transform parameters"
        >
          <ScheduleDisclosure
            title="Named input branches"
            description="Fallback rules for inputs that request a specific branch"
          >
            <PolicyJson
              label="Named input branch policies (JSON)"
              value={draft.build.input_fallback_policy}
              onChange={(v) => {
                try {
                  const valid = decode("ScheduleDefinitionV1", {
                    ...initial,
                    build: { ...initial.build, input_fallback_policy: v },
                  });
                  build({
                    input_fallback_policy: valid.build.input_fallback_policy,
                  });
                  setPolicyError("");
                } catch {
                  setPolicyError("Check the named input branch policy JSON");
                  throw Error("Invalid named input policies");
                }
              }}
            />
          </ScheduleDisclosure>
          <ScheduleDisclosure
            title="External workspace inputs"
            description="Fallback rules for datasets in other workspaces"
          >
            <PolicyJson
              label="Provider input policies (JSON)"
              value={draft.build.provider_fallback_policies}
              onChange={(v) => {
                try {
                  const valid = decode("ScheduleDefinitionV1", {
                    ...initial,
                    build: { ...initial.build, provider_fallback_policies: v },
                  });
                  build({
                    provider_fallback_policies:
                      valid.build.provider_fallback_policies,
                  });
                  setPolicyError("");
                } catch {
                  setPolicyError("Check the provider input policy JSON");
                  throw Error("Invalid provider input policies");
                }
              }}
            />
          </ScheduleDisclosure>
          <ScheduleDisclosure
            title="Transform parameters"
            description="Values passed to transforms when this schedule runs"
          >
            <PolicyJson
              label="Parameter overrides (JSON)"
              value={draft.build.parameters}
              onChange={(v) => {
                const valid = decode("ScheduleDefinitionV1", {
                  ...initial,
                  build: { ...initial.build, parameters: v },
                });
                build({ parameters: valid.build.parameters });
              }}
            />
          </ScheduleDisclosure>
        </ScheduleDisclosure>
      </ScheduleDisclosure>
      {(error || policyError) && (
        <p className="execution-error" role="alert">
          {error || policyError}
        </p>
      )}
      {error && record && onReload && (
        <Button
          disabled={busy}
          onClick={() => {
            const abort = new AbortController();
            saveRequest.current?.abort();
            saveRequest.current = abort;
            setBusy(true);
            void onReload(abort.signal)
              .catch((e) => {
                if (!abort.signal.aborted)
                  setError(e instanceof Error ? e.message : "Reload failed");
              })
              .finally(() => {
                if (!abort.signal.aborted) {
                  setBusy(false);
                  saveAndExit.current = false;
                }
              });
          }}
        >
          Reload saved definition (discard draft)
        </Button>
      )}
      <dialog
        ref={exitDialog}
        className="schedule-exit-dialog"
        aria-labelledby={exitTitle}
        onCancel={(e) => {
          e.preventDefault();
          setConfirmExit(false);
        }}
      >
        <h2 id={exitTitle}>Save schedule changes?</h2>
        <p>Your changes have not been saved.</p>
        <div className="schedule-actions">
          <Button
            onClick={() => {
              setConfirmExit(false);
              cancelButton.current?.focus();
            }}
          >
            Keep editing
          </Button>
          <Button
            onClick={() => {
              setConfirmExit(false);
              onCancel();
            }}
          >
            Discard changes
          </Button>
          <Button
            className="button-primary"
            disabled={
              busy ||
              !!policyError ||
              !!connectionError ||
              !draft.build.targets.length
            }
            onClick={() => {
              setConfirmExit(false);
              saveAndExit.current = true;
              form.current?.requestSubmit();
            }}
          >
            Save and exit
          </Button>
        </div>
      </dialog>
    </form>
  );
}
