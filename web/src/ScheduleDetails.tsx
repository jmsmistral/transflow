import type { ScheduleDefinitionV1 } from "./generated/contracts";
import { Icon } from "./Icons";
import { identity } from "./schedule-model";

type Dataset = ScheduleDefinitionV1["build"]["targets"][number];
export type SchedulePath = (dataset: Dataset) => string;

export function ScheduleDatasets({
  datasets,
  path,
  empty = "None",
}: {
  datasets: readonly Dataset[];
  path: SchedulePath;
  empty?: string;
}) {
  return datasets.length ? (
    <div className="schedule-datasets" role="list">
      {datasets.map((dataset) => (
        <div
          className="schedule-dataset"
          role="listitem"
          key={identity(dataset)}
        >
          <Icon name="table" />
          <span>{path(dataset)}</span>
        </div>
      ))}
    </div>
  ) : (
    <p className="muted">{empty}</p>
  );
}

export function ScheduleBranches({
  definition,
}: {
  definition: ScheduleDefinitionV1;
}) {
  return (
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
    </dl>
  );
}

export function ScheduleFacts({
  definition,
  full = false,
}: {
  definition: ScheduleDefinitionV1;
  full?: boolean;
}) {
  const { build, policies } = definition;
  const source = build.source;
  return (
    <dl className="schedule-details">
      <dt>Code source</dt>
      <dd>
        {source.kind === "git_ref"
          ? `Git ref ${source.ref}`
          : source.kind === "fixed_snapshot"
            ? "Fixed code snapshot"
            : "Current working files"}
        {full && source.kind === "fixed_snapshot" && (
          <code className="schedule-source-id">{source.snapshot_id}</code>
        )}
        {full && (
          <small className="schedule-source-help">
            The code used to build the datasets. A fixed snapshot keeps the
            captured code; a Git ref follows its committed code; working files
            are captured for each occurrence.
          </small>
        )}
      </dd>
      {full && (
        <>
          <dt>Build scope</dt>
          <dd>
            {
              {
                full: "Targets and required ancestors",
                selected: "Selected datasets only",
                between: "Between selected endpoints",
                connecting: "All connecting datasets",
              }[build.build_mode]
            }
          </dd>
        </>
      )}
      <dt>Attempts / retry classes</dt>
      <dd>
        {policies.max_attempts} /{" "}
        {policies.retryable_classes
          .map((value) => value.replaceAll("_", " "))
          .join(", ") || "none"}
      </dd>
      {full && (
        <>
          <dt>Overlapping builds</dt>
          <dd>
            {policies.allow_overlapping_builds
              ? "Allowed with write reservations"
              : "Wait for the active build"}
          </dd>
          <dt>Pending occurrences</dt>
          <dd>{policies.overlap_policy.replaceAll("_", " ")}</dd>
          <dt>Maximum pending</dt>
          <dd>{policies.max_pending}</dd>
          <dt>Missed ticks</dt>
          <dd>{policies.misfire_policy.replaceAll("_", " ")}</dd>
          <dt>Maximum catch-up</dt>
          <dd>{policies.max_catch_up}</dd>
        </>
      )}
      <dt>Event expiry</dt>
      <dd>
        {policies.token_window_seconds === null
          ? "No expiry acknowledged"
          : `${policies.token_window_seconds} s`}
      </dd>
      <dt>Automatic burst limit</dt>
      <dd>
        {policies.max_consecutive_builds} · {policies.minimum_delay_seconds} s
        minimum delay
      </dd>
      {full && (
        <>
          <dt>Force build</dt>
          <dd>{build.force ? "On" : "Off"}</dd>
          <dt>Abort dependent work</dt>
          <dd>{policies.abort_on_failure ? "On" : "Off"}</dd>
          <dt>Require current inputs</dt>
          <dd>{build.require_current ? "On" : "Off"}</dd>
          <dt>Transform timeout</dt>
          <dd>{build.timeout_seconds} s</dd>
          <dt>Validation timeout</dt>
          <dd>{build.validation_timeout_seconds} s</dd>
          {source.kind === "working_tree" && (
            <>
              <dt>Additive registration</dt>
              <dd>{source.allow_additive_sync ? "Allowed" : "Off"}</dd>
            </>
          )}
        </>
      )}
    </dl>
  );
}
