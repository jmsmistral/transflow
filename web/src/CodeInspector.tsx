import { useEffect, useState } from "react";
import type { ApiSourceV1 } from "./generated/contracts";
import type { Workspace } from "./workspace";
import { Button } from "./components";
import { SourceLines } from "./SourceLines";
import { field, versionMetadata } from "./inspectors";
import { sourceDiff } from "./execution-model";
import {
  type Ready,
  type Scope,
  branchSelection,
  message,
  Empty,
  Fallback,
} from "./execution-ui";
async function capturedSource(
  scope: Scope,
  dataset: string,
  origin: string,
): Promise<ApiSourceV1 & { complete: boolean }> {
  const definition = await scope.read(
    "ApiDatasetV1",
    `/api/v1/datasets/${dataset}`,
    { origin_workspace: origin },
  );
  const path = field(definition.producer, "path"),
    source = scope.context.source;
  if (!path || !source)
    throw new Error("Producing source was not retained for this dataset.");
  let offset = 0,
    text = "",
    last: ApiSourceV1;
  do {
    last = await scope.read("ApiSourceV1", `/api/v1/source/${source}`, {
      path,
      offset: String(offset),
      limit: "200",
    });
    if (last.source !== source || last.path !== path || last.offset !== offset)
      throw new Error("Source excerpt does not match this capture.");
    const encoded = new TextEncoder().encode(text + last.text);
    if (encoded.length > 512 * 1024)
      return {
        ...last,
        offset: 0,
        text: new TextDecoder().decode(encoded.slice(0, 512 * 1024)),
        complete: false,
      };
    text += last.text;
    if (last.next_offset === null)
      return { ...last, offset: 0, text, complete: true };
    if (last.next_offset <= offset)
      throw new Error("Source pagination did not advance.");
    offset = last.next_offset;
    if (offset >= 2000 || text.length >= 512 * 1024) break;
    text += "\n";
  } while (offset < 2000 && text.length < 512 * 1024);
  return { ...last, offset: 0, text, complete: false };
}
export function CodeInspector({
  workspace,
  state,
}: {
  workspace: Workspace;
  state: Ready;
}) {
  const dataset = state.value.dataset;
  const [mode, setMode] = useState<
    "Data publish version" | "Latest saved version" | "Diff"
  >("Data publish version");
  const [result, setResult] = useState<{
    key: string;
    producing?: (ApiSourceV1 & { complete: boolean }) | undefined;
    current?: (ApiSourceV1 & { complete: boolean }) | undefined;
    error?: string;
  }>();
  const meta = dataset
    ? versionMetadata(
        dataset,
        state.value.versions,
        state.selection.version,
        state.value.inspection,
      )
    : null;
  const key = JSON.stringify([
    state.value.context.fingerprint,
    dataset?.dataset_id,
    meta?.version,
    mode,
  ]);
  useEffect(() => {
    if (!dataset) return;
    const abort = new AbortController();
    void (async () => {
      if (dataset.origin === "external")
        throw new Error(
          "Producing source is retained by the provider workspace and is unavailable here.",
        );
      let producing, current;
      if (mode !== "Latest saved version") {
        if (!meta?.version)
          throw new Error(
            "No published version is available. Choose Latest saved version to inspect its captured source.",
          );
        const scope = await workspace.scope(
          {
            branch: state.selection.branch,
            dataset: dataset.dataset_id,
            origin: dataset.workspace_id,
            version: meta.version,
          },
          abort.signal,
        );
        producing = await capturedSource(
          scope,
          dataset.dataset_id,
          dataset.workspace_id,
        );
      }
      if (mode !== "Data publish version") {
        const scope = await workspace.scope(
          branchSelection(state),
          abort.signal,
        );
        current = await capturedSource(
          scope,
          dataset.dataset_id,
          dataset.workspace_id,
        );
      }
      return { key, producing, current };
    })().then(
      (value) => {
        if (!abort.signal.aborted) setResult(value);
      },
      (error) => {
        if (!abort.signal.aborted) setResult({ key, error: message(error) });
      },
    );
    return () => abort.abort();
  }, [workspace, state, dataset, key, mode, meta?.version]);
  const shown = result?.key === key ? result : null;
  const source =
    mode === "Latest saved version" ? shown?.current : shown?.producing;
  const resolved =
    meta?.originBranch ?? field(dataset?.head, "resolved_branch");
  const diff =
    mode === "Diff" && shown?.producing?.complete && shown.current?.complete
      ? sourceDiff(shown.producing.text, shown.current.text)
      : null;
  return (
    <div className="execution-inspector code-inspector">
      {mode !== "Latest saved version" && !state.selection.version && (
        <Fallback branch={resolved} requested={state.selection.branch} />
      )}
      <div className="execution-toolbar">
        <strong>{dataset?.path}</strong>
        <div className="execution-switches" aria-label="Code views">
          {(
            ["Data publish version", "Latest saved version", "Diff"] as const
          ).map((name) => (
            <Button
              key={name}
              aria-pressed={mode === name}
              title={
                name === "Latest saved version"
                  ? "Latest validated source capture; excludes unvalidated working-tree edits"
                  : undefined
              }
              onClick={() => setMode(name)}
            >
              {name}
            </Button>
          ))}
        </div>
      </div>
      {source && <div className="source-provenance">{source.path}</div>}
      {shown?.error ? (
        <Empty>Source unavailable: {shown.error}</Empty>
      ) : !shown ? (
        <Empty>Loading retained source…</Empty>
      ) : mode === "Diff" && !diff ? (
        <Empty>
          Diff unavailable for a bounded excerpt. Inspect the two source views
          separately.
        </Empty>
      ) : (
        <div
          className="source-scroll"
          tabIndex={0}
          role="textbox"
          aria-readonly="true"
          aria-multiline="true"
          aria-label={
            mode === "Diff"
              ? "Source diff: producing to current retained definition"
              : "Read-only captured source"
          }
        >
          <SourceLines source={source} current={shown?.current} diff={diff} />
          {source && !source.complete && (
            <p className="bounded-notice">
              Showing a bounded excerpt (maximum 2,000 lines / 512 KiB); use the
              retained snapshot for the complete file.
            </p>
          )}
        </div>
      )}
    </div>
  );
}
