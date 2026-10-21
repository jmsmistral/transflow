import { useEffect, useState } from "react";
import type { ApiAttemptV1, ApiDatasetHistoryV1 } from "./generated/contracts";
import type { Workspace } from "./workspace";
import { field, versionMetadata } from "./inspectors";
import { SourceLines } from "./SourceLines";
import { type Ready, Empty, branchSelection, message } from "./execution-ui";

/** Read actual attempt evidence. A Code read must never invoke a SQL factory. */
export function GeneratedSqlInspector({
  workspace,
  state,
  published,
}: {
  workspace: Workspace;
  state: Ready;
  published: boolean;
}) {
  const dataset = state.value.dataset;
  const [result, setResult] = useState<{
    key: string;
    value?: ApiAttemptV1;
    error?: string;
  }>();
  const key = JSON.stringify([
    state.value.context.fingerprint,
    dataset?.dataset_id,
    published,
    state.selection.version,
  ]);
  useEffect(() => {
    if (!dataset) return;
    const abort = new AbortController();
    void (async () => {
      const meta = versionMetadata(
        dataset,
        state.value.versions,
        state.selection.version,
        state.value.inspection,
      );
      let branch = state.selection.branch;
      let scope = await workspace.scope(branchSelection(state), abort.signal);
      let attempt: string | null | undefined;
      if (published || state.selection.version) {
        attempt = meta.attempt;
        branch =
          meta.originBranch ?? field(dataset.head, "resolved_branch") ?? branch;
        if (!meta.version || !attempt)
          throw new Error(
            "No producing SQL attempt is retained for this publication.",
          );
        scope = await workspace.scope(
          {
            branch,
            version: meta.version,
            dataset: dataset.dataset_id,
            origin: dataset.workspace_id,
          },
          abort.signal,
        );
      } else {
        let history: ApiDatasetHistoryV1 = await scope.read(
          "ApiDatasetHistoryV1",
          `/api/v1/datasets/${dataset.dataset_id}/history`,
          { limit: "1", order: "newest" },
        );
        const fallback =
          meta.originBranch ?? field(dataset.head, "resolved_branch");
        if (!history.entries.length && fallback && fallback !== branch) {
          branch = fallback;
          scope = await workspace.scope({ branch }, abort.signal);
          history = await scope.read(
            "ApiDatasetHistoryV1",
            `/api/v1/datasets/${dataset.dataset_id}/history`,
            { limit: "1", order: "newest" },
          );
        }
        const job = history.entries[0];
        if (!job) throw new Error("This dataset has no retained SQL attempt.");
        attempt = job.attempts.at(-1)?.attempt;
        if (!attempt && job.reused_version && job.original_attempt) {
          attempt = job.original_attempt;
          scope = await workspace.scope(
            {
              branch,
              version: job.reused_version,
              dataset: dataset.dataset_id,
              origin: dataset.workspace_id,
            },
            abort.signal,
          );
        } else {
          scope = await workspace.scope(
            { branch, plan: job.plan },
            abort.signal,
          );
        }
      }
      if (!attempt)
        throw new Error(
          "The latest job has not generated SQL. Earlier SQL has not been substituted.",
        );
      const value = await scope.read(
        "ApiAttemptV1",
        `/api/v1/attempts/${attempt}`,
      );
      if (value.dataset !== dataset.dataset_id || value.attempt !== attempt)
        throw new Error(
          "SQL evidence does not match this dataset and attempt.",
        );
      if (!value.generated_sql)
        throw new Error(
          "SQL was not generated for this attempt, or its execution evidence is no longer retained.",
        );
      return { key, value };
    })().then(
      (value) => {
        if (!abort.signal.aborted) setResult(value);
      },
      (error) => {
        if (!abort.signal.aborted) setResult({ key, error: message(error) });
      },
    );
    return () => abort.abort();
  }, [workspace, state, dataset, key, published]);
  const shown = result?.key === key ? result : null;
  const evidence = shown?.value?.generated_sql;
  const lines = evidence?.sql.split("\n");
  const excerpt = lines?.slice(0, 2000).join("\n");
  return !shown ? (
    <Empty>Loading executed SQL…</Empty>
  ) : shown.error ? (
    <Empty>Generated SQL unavailable: {shown.error}</Empty>
  ) : evidence ? (
    <>
      <div className="source-provenance">
        {published
          ? "SQL that produced this publication"
          : "Latest executed SQL; saved code may have changed"}
        {" · Attempt "}
        {evidence.attempt}
        {" · Source "}
        {evidence.source}
      </div>
      <div
        className="source-scroll"
        tabIndex={0}
        role="textbox"
        aria-readonly
        aria-multiline
        aria-label="Read-only generated SQL"
      >
        <SourceLines
          source={{ text: excerpt ?? "", path: "generated.sql" }}
          current={undefined}
          diff={null}
        />
        {lines && lines.length > 2000 && (
          <p className="bounded-notice">
            Showing the first 2,000 lines of retained SQL.
          </p>
        )}
      </div>
    </>
  ) : null;
}
