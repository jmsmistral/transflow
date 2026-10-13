import { useId } from "react";
import type { ScheduleDefinitionV1 } from "./generated/contracts";
import { Icon } from "./Icons";
import { identity, secondsLabel } from "./schedule-model";

const detailHelp: Record<string, string> = {
  "Data branch":
    "The branch where this schedule publishes its dataset outputs.",
  "Fallback order":
    "When an input is missing on the data branch, try these branches in order. Explicit input policies remain independent.",
  "Code source":
    "The captured code, committed Git ref, or working files used to build datasets. This is separate from the output data branch.",
  "Build scope":
    "Which targets and related producers are included in the build.",
  "Attempts / retry classes":
    "The maximum job attempts and failure classes that are eligible for retry.",
  "Overlapping builds":
    "Whether separate occurrences may build concurrently, subject to write reservations.",
  "Pending occurrences":
    "How new occurrences wait, merge or skip while another build is active.",
  "Maximum pending": "The maximum number of waiting occurrences.",
  "Missed ticks": "How intended times missed during downtime are handled.",
  "Maximum catch-up": "The limit on missed times recovered together.",
  "Event expiry":
    "How long matched trigger evidence remains eligible to combine with other conditions.",
  "Automatic burst limit":
    "The maximum consecutive automatic builds and the minimum delay between them.",
  "Force build":
    "Whether up-to-date target jobs execute again instead of reusing cached outputs.",
  "Abort dependent work": "Whether a required failure stops dependent work.",
  "Require current inputs":
    "Whether boundary inputs must be current before accepting a build.",
  "Transform timeout": "Maximum elapsed time allowed for transform execution.",
  "Validation timeout": "Maximum elapsed time allowed for validation.",
  "Additive registration":
    "Whether validated new output declarations may be registered when capturing working files.",
};
function DetailLabel({ label }: { label: string }) {
  const id = useId();
  return (
    <dt className="schedule-detail-label">
      <span>{label}</span>
      <span className="schedule-info">
        <button
          type="button"
          aria-label={`About ${label.toLowerCase()}`}
          aria-describedby={id}
        >
          <Icon name="info" />
        </button>
        <span id={id} role="tooltip">
          {detailHelp[label]}
        </span>
      </span>
    </dt>
  );
}

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
      <DetailLabel label="Data branch" />
      <dd>{definition.build.data_branch}</dd>
      <DetailLabel label="Fallback order" />
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
      <DetailLabel label="Code source" />
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
          <DetailLabel label="Build scope" />
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
      <DetailLabel label="Attempts / retry classes" />
      <dd>
        {policies.max_attempts} /{" "}
        {policies.retryable_classes
          .map((value) => value.replaceAll("_", " "))
          .join(", ") || "none"}
      </dd>
      {full && (
        <>
          <DetailLabel label="Overlapping builds" />
          <dd>
            {policies.allow_overlapping_builds
              ? "Allowed with write reservations"
              : "Wait for the active build"}
          </dd>
          <DetailLabel label="Pending occurrences" />
          <dd>{policies.overlap_policy.replaceAll("_", " ")}</dd>
          <DetailLabel label="Maximum pending" />
          <dd>{policies.max_pending}</dd>
          <DetailLabel label="Missed ticks" />
          <dd>{policies.misfire_policy.replaceAll("_", " ")}</dd>
          <DetailLabel label="Maximum catch-up" />
          <dd>{policies.max_catch_up}</dd>
        </>
      )}
      <DetailLabel label="Event expiry" />
      <dd>
        {policies.token_window_seconds === null
          ? "No expiry acknowledged"
          : secondsLabel(policies.token_window_seconds)}
      </dd>
      <DetailLabel label="Automatic burst limit" />
      <dd>
        {policies.max_consecutive_builds} ·{" "}
        {secondsLabel(policies.minimum_delay_seconds)} minimum delay
      </dd>
      {full && (
        <>
          <DetailLabel label="Force build" />
          <dd>{build.force ? "On" : "Off"}</dd>
          <DetailLabel label="Abort dependent work" />
          <dd>{policies.abort_on_failure ? "On" : "Off"}</dd>
          <DetailLabel label="Require current inputs" />
          <dd>{build.require_current ? "On" : "Off"}</dd>
          <DetailLabel label="Transform timeout" />
          <dd>{secondsLabel(build.timeout_seconds)}</dd>
          <DetailLabel label="Validation timeout" />
          <dd>{secondsLabel(build.validation_timeout_seconds)}</dd>
          {source.kind === "working_tree" && (
            <>
              <DetailLabel label="Additive registration" />
              <dd>{source.allow_additive_sync ? "Allowed" : "Off"}</dd>
            </>
          )}
        </>
      )}
    </dl>
  );
}
