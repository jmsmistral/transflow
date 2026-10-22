import { useId, useState, type ReactNode } from "react";
import type { ApiContextV1, PlanResultV1 } from "./generated/contracts";
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

export function PlanFallbackBranches({
  plan,
  context,
}: {
  plan: PlanResultV1;
  context: ApiContextV1;
}) {
  return (
    <div className="planner-evidence">
      {plan.warnings.map((warning, index) => (
        <p className="planner-notice" key={index}>
          <Icon name="info" />
          {warning}
        </p>
      ))}
      <section aria-label="Fall-back branches">
        <h3>Fall-back branches</h3>
        <ol className="plan-branches">
          {context.fallback_policy.map((branch) => (
            <li key={branch}>{branch}</li>
          ))}
        </ol>
      </section>
    </div>
  );
}
