import { decode } from "./api/client";
import type {
  ApiSelectionV1,
  ApiLineageNodeV1,
  PlanResultV1,
} from "./generated/contracts";
export const lines = (text: string) => [
  ...new Set(
    text
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean),
  ),
];
export function selectedTargets(nodes: readonly ApiLineageNodeV1[]) {
  return nodes
    .filter((n) => !n.external && n.producer)
    .flatMap((n) => n.paths.slice(0, 1))
    .sort()
    .join("\n");
}
export interface Options {
  targets: string;
  mode: "full" | "selected" | "between" | "connecting";
  boundaries: string;
  exclusions: string;
  refresh: string;
  pins: string;
  parameters: string;
  force: boolean;
  requireCurrent: boolean;
  gitRef: string;
}
export function selection(
  options: Options,
  branch: string,
  fallbacks: readonly string[] | null,
): ApiSelectionV1 {
  const parameters: unknown = JSON.parse(options.parameters || "{}");
  if (
    !parameters ||
    typeof parameters !== "object" ||
    Array.isArray(parameters)
  )
    throw new Error("Parameters must be a JSON object.");
  // JSON parsing admits only the execution JSON grammar; the API validates each declared parameter.
  const request = {
    branch,
    targets: lines(options.targets),
    mode: options.mode,
    boundaries: lines(options.boundaries),
    exclusions: lines(options.exclusions),
    refresh_sources: lines(options.refresh),
    pins: lines(options.pins),
    parameters,
    force: options.force,
    require_current: options.requireCurrent,
    fallbacks,
    ...(options.gitRef.trim() ? { git_ref: options.gitRef.trim() } : {}),
  };
  if (!request.targets.length)
    throw new Error("Choose at least one target dataset.");
  return decode("ApiSelectionV1", request);
}
export function resources(plan: PlanResultV1) {
  const pending = new Set(plan.pending_registrations.map((p) => p.dataset));
  return [
    ...new Map(
      [
        ...plan.writes.map((w) => ({
          identity: `dataset:${plan.workspace}:${w.dataset}`,
          path: w.path,
          pending: pending.has(w.dataset),
        })),
        ...plan.reads.map((r) => ({
          identity: `dataset:${r.origin_workspace ?? plan.workspace}:${r.dataset}`,
          path: r.path,
          pending: false,
        })),
      ].map((r) => [r.identity, r]),
    ).values(),
  ];
}
