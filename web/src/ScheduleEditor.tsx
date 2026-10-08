import { useEffect, useState, useRef, type ReactNode } from "react";
import type {
  ApiDatasetV1,
  ScheduleDefinitionV1,
  ScheduleTriggerV1,
  DatasetKey,
  ScheduleRecordV1,
  SchedulePoliciesV1,
  ApiScheduleClockPreviewV1,
} from "./generated/contracts";
import { Button } from "./components";
import { Icon } from "./Icons";
import { decode } from "./api/client";
import type { Workspace } from "./workspace";
import { identity, keyOf, savedDefinition } from "./schedule-model";

export function Switch({
  label,
  value,
  onChange,
}: {
  label: string;
  value: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="schedule-switch">
      <button
        type="button"
        role="switch"
        aria-checked={value}
        aria-label={label}
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
}: {
  value: ScheduleTriggerV1;
  onChange: (v: ScheduleTriggerV1) => void;
  datasets: readonly ApiDatasetV1[];
  schedules: readonly ScheduleRecordV1[];
  branch: string;
  depth?: number;
}) {
  return (
    <fieldset className="schedule-condition">
      <legend>{depth === 0 ? "When to build" : "Condition"}</legend>
      <Field label="Condition type">
        <select
          value={value.kind}
          onChange={(e) =>
            onChange(newTrigger(e.target.value, datasets, branch))
          }
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
      {value.kind !== "manual" &&
        value.kind !== "and" &&
        value.kind !== "or" && (
          <Field label="Condition ID">
            <input
              value={value.id}
              onChange={(e) => onChange({ ...value, id: e.target.value })}
            />
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
                  Unresolved dataset
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
          <Field label="Input binding">
            <select
              value={value.payload_mode}
              onChange={(e) =>
                onChange({
                  ...value,
                  payload_mode:
                    e.target.value === "signal_only" ? "signal_only" : "pin",
                })
              }
            >
              <option value="pin">Pin the exact published version</option>
              <option value="signal_only">Signal only</option>
            </select>
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
            <div className="schedule-child" key={i}>
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
              />
              {value.children.length > 2 && (
                <Button
                  onClick={() =>
                    onChange({
                      ...value,
                      children: value.children.filter((_, j) => j !== i),
                    })
                  }
                >
                  Remove condition
                </Button>
              )}
            </div>
          ))}
          <Button
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
            <Icon name="new" /> Add condition
          </Button>
        </>
      )}
    </fieldset>
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
  onDefaults,
  onReload,
}: {
  workspace: Workspace;
  initial: ScheduleDefinitionV1;
  record: ScheduleRecordV1 | null;
  datasets: readonly ApiDatasetV1[];
  schedules: readonly ScheduleRecordV1[];
  onSave: (d: ScheduleDefinitionV1, signal: AbortSignal) => Promise<void>;
  onCancel: () => void;
  onDefaults: () => Promise<ScheduleDefinitionV1>;
  onReload?: (signal: AbortSignal) => Promise<void>;
}) {
  const [draft, setDraft] = useState(initial),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [clock, setClock] = useState<ApiScheduleClockPreviewV1 | null>(null),
    [clockError, setClockError] = useState("");
  const [policyError, setPolicyError] = useState("");
  const [policyGeneration, setPolicyGeneration] = useState(0);
  const build = (patch: Partial<ScheduleDefinitionV1["build"]>) =>
    setDraft((d) => ({ ...d, build: { ...d.build, ...patch } }));
  const policies = (patch: Partial<SchedulePoliciesV1>) =>
    setDraft((d) => ({ ...d, policies: { ...d.policies, ...patch } }));
  useEffect(() => {
    const abort = new AbortController();
    const timer = setTimeout(() => {
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
  }, [draft, workspace]);
  const saveRequest = useRef<AbortController | null>(null);
  useEffect(() => () => saveRequest.current?.abort(), []);
  const source = draft.build.source;
  return (
    <form
      className="schedule-editor"
      onSubmit={(e) => {
        e.preventDefault();
        if (policyError) return;
        setBusy(true);
        setError("");
        const abort = new AbortController();
        saveRequest.current?.abort();
        saveRequest.current = abort;
        void Promise.resolve()
          .then(() =>
            onSave(decode("ScheduleDefinitionV1", draft), abort.signal),
          )
          .catch((e) => {
            if (!abort.signal.aborted)
              setError(e instanceof Error ? e.message : "Schedule save failed");
          })
          .finally(() => {
            if (!abort.signal.aborted) setBusy(false);
          });
      }}
    >
      <header>
        <h3>{record ? "Edit schedule" : "Create schedule"}</h3>
        <p className="muted">
          Save replaces the current definition. Accepted work keeps its captured
          settings.
        </p>
      </header>
      <Field label="Schedule name">
        <input
          required
          value={draft.name}
          onChange={(e) => setDraft({ ...draft, name: e.target.value })}
        />
      </Field>
      <Field label="Description">
        <textarea
          rows={2}
          value={draft.description}
          onChange={(e) => setDraft({ ...draft, description: e.target.value })}
        />
      </Field>
      <DatasetChoices
        label="Datasets to build"
        value={draft.build.targets}
        datasets={datasets}
        local
        onChange={(targets) => build({ targets })}
      />
      <Field label="Build scope">
        <select
          value={draft.build.build_mode}
          onChange={(e) => {
            const mode = e.target.value;
            build({
              build_mode:
                mode === "selected"
                  ? "selected"
                  : mode === "between"
                    ? "between"
                    : mode === "connecting"
                      ? "connecting"
                      : "full",
            });
          }}
        >
          <option value="full">Targets and required ancestors</option>
          <option value="selected">Selected datasets only</option>
          <option value="between">Between selected endpoints</option>
          <option value="connecting">All connecting datasets</option>
        </select>
      </Field>
      <Field label="Data branch">
        <input
          required
          value={draft.build.data_branch}
          onChange={(e) => build({ data_branch: e.target.value })}
        />
      </Field>
      <Field label="Fallback branches (in order)">
        <input
          placeholder="main, stable"
          value={draft.build.fallback_branches.join(", ")}
          onChange={(e) =>
            build({
              fallback_branches: e.target.value
                .split(",")
                .map((v) => v.trim())
                .filter(Boolean),
            })
          }
        />
      </Field>
      <Field label="Source selection">
        <select
          value={source.kind}
          onChange={(e) => {
            switch (e.target.value) {
              case "git_ref":
                build({ source: { kind: "git_ref", ref: "refs/heads/main" } });
                break;
              case "fixed_snapshot":
                build({
                  source: {
                    kind: "fixed_snapshot",
                    snapshot_id:
                      workspace.snapshot().kind === "ready"
                        ? initial.build.source.kind === "fixed_snapshot"
                          ? initial.build.source.snapshot_id
                          : ""
                        : "",
                  },
                });
                break;
              default:
                build({
                  source: { kind: "working_tree", allow_additive_sync: false },
                });
            }
          }}
        >
          <option value="fixed_snapshot">
            Fixed validated source snapshot
          </option>
          <option value="git_ref">Named clean Git ref</option>
          <option value="working_tree">Mutable working tree</option>
        </select>
      </Field>
      {source.kind === "git_ref" && (
        <Field label="Source Git ref">
          <input
            value={source.ref}
            onChange={(e) =>
              build({ source: { ...source, ref: e.target.value } })
            }
          />
        </Field>
      )}
      {source.kind === "fixed_snapshot" && (
        <Field label="Source snapshot">
          <input
            value={source.snapshot_id}
            onChange={(e) =>
              build({ source: { ...source, snapshot_id: e.target.value } })
            }
          />
        </Field>
      )}
      {source.kind === "working_tree" && (
        <>
          <p className="schedule-notice">
            <Icon name="info" /> Each occurrence captures the current working
            files. Later edits can change automatic builds.
          </p>
          <Switch
            label="Allow additive dataset registration"
            value={source.allow_additive_sync}
            onChange={(allow_additive_sync) =>
              build({ source: { ...source, allow_additive_sync } })
            }
          />
        </>
      )}
      <Button
        disabled={busy}
        onClick={() => {
          void onDefaults()
            .then((d) => {
              build({
                source: d.build.source,
                input_fallback_policy: d.build.input_fallback_policy,
                provider_fallback_policies: d.build.provider_fallback_policies,
                fallback_branches: d.build.fallback_branches,
              });
              setPolicyError("");
              setPolicyGeneration((n) => n + 1);
            })
            .catch((e) =>
              setError(e instanceof Error ? e.message : "Defaults unavailable"),
            );
        }}
      >
        Use workspace source and input defaults
      </Button>
      <TriggerEditor
        value={draft.trigger}
        onChange={(trigger) => setDraft({ ...draft, trigger })}
        datasets={datasets}
        schedules={schedules}
        branch={draft.build.data_branch}
      />
      {clock && clock.leaves.length > 0 && (
        <section className="schedule-clock">
          <h4>Next intended times</h4>
          {clock.leaves.map((l) => (
            <div key={l.id}>
              <strong>{l.id}</strong>
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
        </section>
      )}
      {clockError && (
        <p role="status" className="muted">
          {clockError}
        </p>
      )}
      <details>
        <summary>Advanced options</summary>
        <Switch
          label="Force build on up-to-date datasets"
          value={draft.build.force}
          onChange={(force) => build({ force })}
        />
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
            onChange={(e) => policies({ max_attempts: e.target.valueAsNumber })}
          />
        </Field>
        <fieldset>
          <legend>Retryable infrastructure failures</legend>
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
                      : draft.policies.retryable_classes.filter((v) => v !== c),
                  })
                }
              />
              {c.replaceAll("_", " ")}
            </label>
          ))}
          <small>
            Authentication, protocol, input integrity and data-quality failures
            are never retried by these options.
          </small>
        </fieldset>
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
        <Field label="Maximum pending occurrences">
          <input
            type="number"
            min={1}
            max={100}
            value={draft.policies.max_pending}
            onChange={(e) => policies({ max_pending: e.target.valueAsNumber })}
          />
        </Field>
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
        <Field label="Maximum catch-up ticks">
          <input
            type="number"
            min={1}
            max={100}
            value={draft.policies.max_catch_up}
            onChange={(e) => policies({ max_catch_up: e.target.valueAsNumber })}
          />
        </Field>
        <Switch
          label="No event expiry (explicit acknowledgment)"
          value={draft.policies.acknowledge_no_expiry}
          onChange={(v) =>
            policies({
              acknowledge_no_expiry: v,
              token_window_seconds: v ? null : 86400,
            })
          }
        />
        {draft.policies.token_window_seconds !== null && (
          <Field label="Event window (seconds)">
            <input
              type="number"
              min={1}
              value={draft.policies.token_window_seconds}
              onChange={(e) =>
                policies({ token_window_seconds: e.target.valueAsNumber })
              }
            />
          </Field>
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
        <Field label="Minimum automatic delay (seconds)">
          <input
            type="number"
            min={0}
            value={draft.policies.minimum_delay_seconds}
            onChange={(e) =>
              policies({ minimum_delay_seconds: e.target.valueAsNumber })
            }
          />
        </Field>
        <Field label="Job deadline (seconds; 0 disables)">
          <input
            type="number"
            min={0}
            value={draft.build.timeout_seconds}
            onChange={(e) => build({ timeout_seconds: e.target.valueAsNumber })}
          />
        </Field>
        <Field label="Validation deadline (seconds; 0 disables)">
          <input
            type="number"
            min={0}
            value={draft.build.validation_timeout_seconds}
            onChange={(e) =>
              build({ validation_timeout_seconds: e.target.valueAsNumber })
            }
          />
        </Field>
        <DatasetChoices
          label="Read boundaries"
          value={draft.build.boundaries}
          datasets={datasets}
          onChange={(boundaries) => build({ boundaries })}
        />
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
        <PolicyJson
          key={`input-${policyGeneration}`}
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
        <PolicyJson
          key={`providers-${policyGeneration}`}
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
        <PolicyJson
          key={`parameters-${policyGeneration}`}
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
      </details>
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
                if (!abort.signal.aborted) setBusy(false);
              });
          }}
        >
          Reload saved definition (discard draft)
        </Button>
      )}
      <footer className="schedule-actions">
        <Button disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="submit"
          className="button-primary"
          disabled={busy || !!policyError || !draft.build.targets.length}
        >
          {busy ? "Saving…" : "Save schedule"}
        </Button>
      </footer>
    </form>
  );
}
