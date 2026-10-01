import type { ReactNode } from "react";
import type {
  ApiHistoryJobV1,
  ApiVersionsV1,
  ExecutionJsonV1,
} from "./generated/contracts";
import type { Workspace, WorkspaceState } from "./workspace";
import { Icon } from "./Icons";
export type Ready = Extract<WorkspaceState, { kind: "ready" }>;
export type Scope = Awaited<ReturnType<Workspace["scope"]>>;
export type Version = ApiVersionsV1["entries"][number];
export function message(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Retained evidence is unavailable.";
}
export function items(
  value: ExecutionJsonV1 | undefined,
): readonly ExecutionJsonV1[] {
  return Array.isArray(value) ? value : [];
}
function isObject(
  value: ExecutionJsonV1 | undefined,
): value is { readonly [key: string]: ExecutionJsonV1 } {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
export function property(
  value: ExecutionJsonV1 | undefined,
  name: string,
): ExecutionJsonV1 | undefined {
  return isObject(value) ? value[name] : undefined;
}

export function Evidence({
  children,
}: {
  children: ExecutionJsonV1 | undefined;
}) {
  return (
    <pre className="execution-json">
      {JSON.stringify(children ?? null, null, 2)}
    </pre>
  );
}
export function Empty({ children }: { children: ReactNode }) {
  return (
    <div className="execution-empty">
      <Icon name="info" />
      <span>{children}</span>
    </div>
  );
}
export function Fallback({
  branch,
  requested,
}: {
  branch: string | undefined;
  requested: string;
}) {
  return branch && branch !== requested ? (
    <div className="preview-fallback-banner" role="status">
      <Icon name="branch" />
      Could not find data on current branch, so showing data from branch{" "}
      <strong>{branch}</strong>
    </div>
  ) : null;
}
export function branchSelection(state: Ready) {
  return {
    branch: state.selection.branch,
    ...(state.selection.fallback ? { fallback: state.selection.fallback } : {}),
  };
}
export interface HistoryResult {
  entries: readonly ApiHistoryJobV1[];
  next: string | null;
}
export function Metric({
  label,
  value,
  title,
}: {
  label: string;
  value: string;
  title?: string;
}) {
  return (
    <div className="metric" title={title}>
      <small>{label}</small>
      <strong>{value}</strong>
    </div>
  );
}
export function Status({ state }: { state: string }) {
  return (
    <span
      className={`execution-status status-${state.toLowerCase()}`}
      aria-label={state}
    >
      {state === "SUCCEEDED"
        ? "✓"
        : state === "FAILED"
          ? "×"
          : state === "CACHED"
            ? "↻"
            : "•"}
    </span>
  );
}
