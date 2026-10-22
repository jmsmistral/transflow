import { useId, useState, type ReactNode } from "react";
import type { PlanResultV1 } from "./generated/contracts";
import { Icon } from "./Icons";
import { formatSize } from "./format-size";

type Write = PlanResultV1["writes"][number];

// These labels describe the accepted-plan prerequisites, not a predicted hit.
export function cacheReadiness(plan: PlanResultV1, write: Write): string {
  if (plan.force) return "Will run · forced build";
  if (plan.source_decisions?.[write.dataset]?.executes)
    return "Will run · source refresh";
  if (!write.declaration) return "Cache policy unavailable";
  if (write.declaration.cache === null) return "Cache policy unavailable";
  if (write.declaration.cache === "never") return "Will run · caching disabled";
  return write.bindings.some((binding) => binding.kind === "planned")
    ? "Cache checked after upstream jobs finish"
    : "Cache checked when the build starts";
}

function Expansion({
  title,
  label,
  subtitle,
  children,
}: {
  title: string;
  label?: string;
  subtitle?: string;
  children: ReactNode;
}) {
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  return (
    <section className="plan-expansion">
      <button
        type="button"
        className="plan-expansion-toggle"
        aria-label={label ?? title}
        aria-expanded={expanded}
        aria-controls={id}
        onClick={() => setExpanded(!expanded)}
      >
        <span>
          <strong>{title}</strong>
          {subtitle && <small>{subtitle}</small>}
        </span>
        <span className="plan-expansion-action" aria-hidden="true">
          {expanded ? "Hide details" : "Show details"}
        </span>
      </button>
      <div
        id={id}
        className="plan-expansion-content"
        data-expanded={expanded}
        aria-hidden={!expanded}
        inert={!expanded}
      >
        <div>
          <div className="plan-expansion-body">{children}</div>
        </div>
      </div>
    </section>
  );
}

function Facts({
  items,
}: {
  items: readonly (readonly [string, ReactNode])[];
}) {
  return (
    <dl className="plan-facts">
      {items.map(([name, value]) => (
        <div key={name}>
          <dt>{name}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function duration(seconds: string) {
  const value = BigInt(seconds);
  if (value === 0n) return "No limit";
  if (value % 3600n === 0n) return `${value / 3600n} h`;
  if (value % 60n === 0n) return `${value / 60n} min`;
  return `${value} s`;
}

export function PlanResources({ plan }: { plan: PlanResultV1 }) {
  return (
    <div className="planner-resource-list">
      {plan.writes.map((write) => {
        const declaration = write.declaration;
        const checks = declaration
          ? [
              ...declaration.output.checks.map((check) => ({
                ...check,
                subject: "Output",
              })),
              ...declaration.inputs.flatMap((input) =>
                input.checks.map((check) => ({
                  ...check,
                  subject: `Input · ${input.alias}`,
                })),
              ),
            ]
          : [];
        return (
          <Expansion
            key={write.job}
            title={write.path}
            label={`Details for ${write.path}`}
            subtitle={cacheReadiness(plan, write)}
          >
            {!!write.bindings.length && <h4>Inputs</h4>}
            {write.bindings.map((binding) => {
              const read = plan.reads.find(
                (item) =>
                  item.consumer === write.dataset &&
                  item.alias === binding.alias,
              );
              const path =
                plan.writes.find((item) => item.dataset === binding.dataset)
                  ?.path ??
                read?.path ??
                binding.dataset;
              return (
                <div className="plan-detail-card" key={binding.alias}>
                  <strong>{path}</strong>
                  <small>
                    {binding.kind === "planned"
                      ? "From this build"
                      : `Published${read ? ` · ${read.resolved_branch}` : ""}`}
                  </small>
                </div>
              );
            })}
            <h4>Checks</h4>
            {checks.map((check, index) => (
              <div className="plan-check-card" key={index}>
                <div>
                  <strong>{check.name}</strong>
                  <small>{check.subject}</small>
                </div>
                <span
                  className="plan-policy"
                  title={
                    check.on_error === "FAIL"
                      ? "A failed check blocks publication"
                      : "A failed check records a warning"
                  }
                >
                  {check.on_error === "FAIL" ? "Required" : "Warning"}
                </span>
              </div>
            ))}
            {!checks.length && (
              <p className="muted">
                {declaration
                  ? "No declared data checks"
                  : "Check definitions unavailable"}
              </p>
            )}
            {!!Object.keys(write.parameters).length && (
              <>
                <h4>Parameters</h4>
                <Facts
                  items={Object.entries(write.parameters).map(
                    ([name, value]) => [name, JSON.stringify(value)],
                  )}
                />
              </>
            )}
            <h4>Execution</h4>
            <Facts
              items={[
                [
                  "Engine",
                  declaration?.engine === "polars"
                    ? "Polars"
                    : declaration?.engine === "duckdb"
                      ? "DuckDB"
                      : "Unavailable",
                ],
                ["Run timeout", duration(write.resources.timeout_seconds)],
                [
                  "Check timeout",
                  duration(write.resources.validation_timeout_seconds),
                ],
                ["Threads", write.resources.worker_threads],
                [
                  "Memory",
                  write.resources.memory_budget_mib
                    ? formatSize(
                        (
                          BigInt(write.resources.memory_budget_mib) * 1048576n
                        ).toString(),
                      )
                    : "No Transflow cap",
                ],
              ]}
            />
          </Expansion>
        );
      })}
    </div>
  );
}

const freshnessLabels = {
  Current: "Up-to-date",
  Stale: "Out-of-date",
  Unknown: "Unavailable",
};

const reasonLabels: Readonly<Record<string, string>> = {
  NEVER_BUILT: "No published version exists",
  INPUT_VERSION_CHANGED: "An input version changed",
  CODE_CHANGED: "Transform code changed",
  ENVIRONMENT_CHANGED: "The execution environment changed",
  PARAMETERS_CHANGED: "Parameter values changed",
  CHECK_DEFINITION_CHANGED: "Check definitions changed",
  EXECUTION_POLICY_CHANGED: "Execution policy changed",
  SOURCE_REFRESH_DUE: "A source refresh is due",
  ARTIFACT_MISSING: "Published data files are missing",
  ARTIFACT_CORRUPT: "Published data files are corrupt",
  ANCESTOR_STALE: "An upstream dataset needs updating",
  BOUNDARY_FALLBACK: "An input uses a fallback branch",
  CATALOG_ADDITION_PENDING: "This dataset needs catalogue registration",
  SOURCE_DEFINITION_UNAVAILABLE: "The source definition is unavailable",
  BOUNDARY_STALE: "Published input data is unavailable",
  CACHE_DISABLED: "Caching is disabled",
  CACHE_MATCH: "Retained computation matches this context",
  METADATA_UNKNOWN: "Freshness comparison is unavailable",
};

export function PlanDetails({ plan }: { plan: PlanResultV1 }) {
  return (
    <div className="planner-evidence">
      {plan.warnings.map((warning, index) => (
        <p className="planner-notice" key={index}>
          <Icon name="info" />
          {warning}
        </p>
      ))}
      <Expansion title="Plan details">
        <Facts
          items={[
            ["Branch", plan.branch],
            ["Jobs in scope", plan.writes.length],
            ["Published input bindings", plan.reads.length],
          ]}
        />
        {!!plan.reads.length && (
          <>
            <h4>Published inputs</h4>
            {plan.reads.map((read) => (
              <div
                className="plan-detail-card"
                key={`${read.consumer}:${read.alias}`}
              >
                <strong>{read.path}</strong>
                <Facts
                  items={[
                    ["Used by", read.consumer_path],
                    ["Branch", read.resolved_branch],
                    ["Resolution", read.resolution],
                    [
                      "Origin",
                      read.origin_workspace &&
                      read.origin_workspace !== plan.workspace
                        ? "External workspace"
                        : "Local workspace",
                    ],
                  ]}
                />
              </div>
            ))}
          </>
        )}
        {!!plan.pending_registrations.length && (
          <>
            <h4>New datasets</h4>
            <p className="muted">Registered when you run this build.</p>
            {plan.pending_registrations.map((item) => (
              <p key={item.dataset}>{item.path}</p>
            ))}
          </>
        )}
        {!!Object.keys(plan.source_decisions ?? {}).length && (
          <>
            <h4>Source refresh</h4>
            {Object.entries(plan.source_decisions ?? {}).map(
              ([dataset, decision]) => (
                <div className="plan-detail-card" key={dataset}>
                  <strong>
                    {plan.writes.find((item) => item.dataset === dataset)
                      ?.path ??
                      plan.reads.find((item) => item.dataset === dataset)
                        ?.path ??
                      dataset}
                  </strong>
                  <Facts
                    items={[
                      [
                        "Action",
                        decision.executes ? "Refresh source" : "Reuse source",
                      ],
                      ["Reason", decision.reason],
                    ]}
                  />
                </div>
              ),
            )}
          </>
        )}
        {!!plan.freshness.length && (
          <>
            <h4>Freshness</h4>
            <p className="muted">Current branch versions, before this build.</p>
            {plan.freshness.map((item) => (
              <Expansion
                key={item.identity}
                title={item.path ?? item.identity}
                label={`Freshness for ${item.path ?? item.identity}`}
              >
                <Facts
                  items={[
                    ["Input data", freshnessLabels[item.direct_data]],
                    ["Transform code", freshnessLabels[item.direct_logic]],
                    ["Upstream datasets", freshnessLabels[item.inherited]],
                  ]}
                />
                {item.reasons.map((reason, index) => (
                  <p className="plan-reason" key={index} title={reason.message}>
                    {reasonLabels[reason.code] ?? reason.message}
                    {reason.path.length > 1 && (
                      <small>{reason.path.join(" → ")}</small>
                    )}
                  </p>
                ))}
              </Expansion>
            ))}
          </>
        )}
      </Expansion>
    </div>
  );
}
