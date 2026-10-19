import type { ExecutionJsonV1 } from "./generated/contracts";
import { field } from "./inspectors";
import { property } from "./execution-ui";

export interface HealthPoint {
  label: string;
  tone: "good" | "warn" | "bad" | "neutral";
  at?: string | undefined;
  checks?: readonly ExecutionJsonV1[] | undefined;
  note?: string | undefined;
}
/** Aggregate recorded results; absent checks and unknown policies never imply PASS. */
export function checkStatus(checks: readonly ExecutionJsonV1[]): HealthPoint {
  if (!checks.length) return { label: "No checks", tone: "neutral" };
  let warning = false,
    unknown = false;
  for (const check of checks) {
    const outcome = field(check, "outcome");
    const severity =
      field(property(check, "policy"), "severity") ??
      field(property(check, "metrics"), "severity");
    if (outcome === "ERROR" || (outcome === "VIOLATION" && severity === "FAIL"))
      return { label: "Failed", tone: "bad" };
    if (outcome === "VIOLATION" && severity === "WARN") warning = true;
    else if (outcome !== "PASS") unknown = true;
  }
  return unknown
    ? { label: "Unavailable", tone: "neutral" }
    : warning
      ? { label: "Warning", tone: "warn" }
      : { label: "Passed", tone: "good" };
}
export function buildStatus(state: string): HealthPoint {
  if (state === "SUCCEEDED" || state === "CACHED")
    return { label: state === "CACHED" ? "Cached" : "Succeeded", tone: "good" };
  if (state === "FAILED") return { label: "Failed", tone: "bad" };
  return {
    label: state.slice(0, 1) + state.slice(1).toLowerCase(),
    tone: "neutral",
  };
}
