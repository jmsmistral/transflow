import { useEffect, useState } from "react";
import type {
  ApiDatasetV1,
  ApiHistoryJobV1,
  ApiVersionsV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { Icon } from "./Icons";
import { field } from "./inspectors";
import { seconds, timestamp } from "./execution-model";
import {
  type Ready,
  type Version,
  Empty,
  Evidence,
  Status,
  items,
  property,
  message,
} from "./execution-ui";
import { useBuildEvidence } from "./build-evidence";
import { BuildModal } from "./BuildReport";
import { Logs } from "./BuildInspectors";

export function JobHistoryDetail({
  workspace,
  state,
  job,
  dataset,
}: {
  workspace: Workspace;
  state: Ready;
  job: ApiHistoryJobV1;
  dataset: ApiDatasetV1;
}) {
  const evidence = useBuildEvidence(workspace, state, job);
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState("Overview");
  const [attemptId, setAttemptId] = useState<string>();
  const [version, setVersion] = useState<Version>();
  const [versionError, setVersionError] = useState("");
  const result = evidence.result;
  const retained = items(property(result?.report, "jobs")).find(
    (j) => field(j, "id") === job.id,
  );
  const timing = result?.timeline.jobs.find((j) => j.job === job.id);
  const attempts = timing?.attempts ?? job.attempts;
  const selectedAttempt =
    attempts.find((a) => a.attempt === attemptId) ?? attempts.at(-1);
  const attemptEvidence = items(property(retained, "attempts")).find(
    (a) => field(a, "id") === selectedAttempt?.attempt,
  );
  const versionId =
    field(retained, "version") ?? job.produced_version ?? job.reused_version;
  useEffect(() => {
    if (!versionId) return;
    const abort = new AbortController();
    void (async () => {
      const scope = await workspace.scope(
        {
          branch: state.selection.branch,
          version: versionId,
          dataset: dataset.dataset_id,
          origin: dataset.workspace_id,
        },
        abort.signal,
      );
      let value: Version | undefined;
      let cursor: string | null = null;
      for (let pageNumber = 0; pageNumber < 5; pageNumber++) {
        const page: ApiVersionsV1 = await scope.read(
          "ApiVersionsV1",
          `/api/v1/datasets/${dataset.dataset_id}/versions`,
          {
            origin_workspace: dataset.workspace_id,
            limit: "200",
            ...(cursor ? { cursor } : {}),
          },
        );
        value = page.entries.find((v) => v.version === versionId);
        if (value || !page.next_cursor) break;
        cursor = page.next_cursor;
      }
      if (!value)
        throw new Error(
          "Published version metadata was not found in the first 1,000 retained versions.",
        );
      if (!abort.signal.aborted) {
        setVersion(value);
        setVersionError("");
      }
    })().catch((error) => {
      if (!abort.signal.aborted) setVersionError(message(error));
    });
    return () => abort.abort();
  }, [workspace, state, dataset.dataset_id, dataset.workspace_id, versionId]);
  const output =
    version?.version === versionId && version.origin === "local"
      ? version
      : undefined;
  const jobState = timing?.state ?? job.state;
  const failure = field(attemptEvidence, "failure_class");
  return (
    <div className="job-detail">
      <nav className="job-tabs" aria-label="Job detail views">
        {["Overview", "Logs", "Files", "Metadata", "Schema", "Job spec"].map(
          (name) => (
            <Button
              key={name}
              aria-pressed={tab === name}
              onClick={() => setTab(name)}
            >
              {name}
            </Button>
          ),
        )}
      </nav>
      {evidence.error && (
        <p role="alert" className="execution-error">
          Job refresh failed: {evidence.error}{" "}
          <Button onClick={evidence.refresh}>Retry</Button>
        </p>
      )}
      {!result ? (
        <Empty>Loading retained job details…</Empty>
      ) : tab === "Overview" ? (
        <div className="job-overview">
          <section>
            <h3>Transaction details</h3>
            <dl className="job-facts">
              <dt>Status</dt>
              <dd>
                {versionId
                  ? jobState === "CACHED"
                    ? "Reused"
                    : "Committed"
                  : "No version published"}
              </dd>
              <dt>Version ID</dt>
              <dd>{versionId ?? "—"}</dd>
              <dt>Published</dt>
              <dd>{timestamp(output?.published_at_us ?? null)}</dd>
              <dt>Files</dt>
              <dd>{output?.file_count ?? "Unavailable"}</dd>
              <dt>Rows</dt>
              <dd>{output?.row_count ?? "Unavailable"}</dd>
              <dt>Size of files</dt>
              <dd>{output ? `${output.byte_count} bytes` : "Unavailable"}</dd>
            </dl>
            {versionError && <p role="status">{versionError}</p>}
          </section>
          <section>
            <div className="job-heading">
              <h3>Job details</h3>
              <Button
                className="build-report-link"
                onClick={() => setOpen(true)}
              >
                <Icon name="build" /> View build report <Icon name="external" />
              </Button>
            </div>
            <dl className="job-facts">
              <dt>Status</dt>
              <dd>
                <Status state={jobState} /> {jobState}{" "}
                <small>
                  (Part of a <Status state={result.timeline.state} />{" "}
                  {result.timeline.state} build)
                </small>
              </dd>
              <dt>Duration</dt>
              <dd>
                {jobState === "CACHED"
                  ? "Cached reuse · no execution"
                  : seconds(timing?.duration_ns ?? job.duration_ns)}
              </dd>
              <dt>Start time</dt>
              <dd>{timestamp(attempts[0]?.started_us ?? null)}</dd>
              <dt>End time</dt>
              <dd>{timestamp(attempts.at(-1)?.finished_us ?? null)}</dd>
              <dt>Started by</dt>
              <dd>{field(result.report, "requested_by") ?? "Unavailable"}</dd>
              <dt>Job ID</dt>
              <dd>{job.id}</dd>
              <dt>Build ID</dt>
              <dd>{job.build}</dd>
              <dt>Attempts</dt>
              <dd>{attempts.length}</dd>
            </dl>
            {failure && (
              <p role="status" className="execution-error">
                Failure: {failure}
              </p>
            )}
          </section>
        </div>
      ) : tab === "Logs" ? (
        <>
          {selectedAttempt ? (
            <>
              <div className="execution-toolbar">
                <label>
                  Attempt{" "}
                  <select
                    value={selectedAttempt.attempt}
                    onChange={(e) => setAttemptId(e.target.value)}
                  >
                    {attempts.map((a) => (
                      <option key={a.attempt} value={a.attempt}>
                        #{a.number} · {a.state}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <Logs
                key={selectedAttempt.attempt}
                scope={result.scope}
                attempt={selectedAttempt.attempt}
              />
            </>
          ) : (
            <Empty>No execution attempt; logs are unavailable.</Empty>
          )}
        </>
      ) : tab === "Files" ? (
        <div className="job-tab-content">
          {output ? (
            <dl className="job-facts">
              <dt>Artifact digest</dt>
              <dd>{output.artifact}</dd>
              <dt>Files</dt>
              <dd>{output.file_count ?? "Unavailable"}</dd>
              <dt>Physical bytes</dt>
              <dd>{output.byte_count}</dd>
            </dl>
          ) : (
            <Empty>No retained output file metadata for this job.</Empty>
          )}
        </div>
      ) : tab === "Schema" ? (
        <div className="job-tab-content">
          {output?.schema ? (
            <Evidence>{output.schema}</Evidence>
          ) : (
            <Empty>No retained output schema for this job.</Empty>
          )}
        </div>
      ) : tab === "Metadata" ? (
        <div className="job-tab-content">
          <h3>Job evidence</h3>
          <Evidence>{retained}</Evidence>
        </div>
      ) : (
        <div className="job-tab-content">
          <h3>Retained job specification</h3>
          <Evidence>
            {items(property(property(result.report, "plan"), "writes")).find(
              (w) => field(w, "dataset") === dataset.dataset_id,
            )}
          </Evidence>
          <h3>Inputs and bindings</h3>
          <Evidence>{property(retained, "inputs")}</Evidence>
          <h3>Captured parameters</h3>
          <Evidence>
            {property(
              property(property(result.report, "plan"), "context"),
              "parameters",
            )}
          </Evidence>
        </div>
      )}
      {open && (
        <BuildModal
          {...evidence}
          dataset={dataset}
          onClose={() => setOpen(false)}
          onOpenSchedule={(id) => {
            setOpen(false);
            workspace.requestSchedule(id);
          }}
        />
      )}
    </div>
  );
}
