import { useEffect, useState } from "react";
import type {
  ApiExecutionTimelineV1,
  ApiHistoryJobV1,
  ApiJobTimingV1,
  ExecutionJsonV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { type Scope, type Ready, message } from "./execution-ui";

export const terminalBuild = (state: string) =>
  ["SUCCEEDED", "FAILED", "CANCELED", "INTERRUPTED"].includes(state);
export interface BuildEvidence {
  timeline: ApiExecutionTimelineV1;
  report: ExecutionJsonV1;
  scope: Scope;
}
/** Refresh a single plan-bound snapshot at a time; never merge runtime revisions. */
export function useBuildEvidence(
  workspace: Workspace,
  state: Ready,
  job: Pick<ApiHistoryJobV1, "build" | "plan" | "source" | "build_state">,
) {
  const [result, setResult] = useState<BuildEvidence>();
  const [error, setError] = useState("");
  const [revision, refresh] = useState(0);
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    let active = !terminalBuild(job.build_state);
    async function read() {
      try {
        const scope = await workspace.scope(
          { branch: state.selection.branch, plan: job.plan },
          abort.signal,
        );
        const [timeline, report] = await Promise.all([
          scope.read(
            "ApiExecutionTimelineV1",
            `/api/v1/builds/${job.build}/timeline`,
          ),
          scope.read("ExecutionJsonV1", `/api/v1/builds/${job.build}`),
        ]);
        if (
          timeline.build !== job.build ||
          timeline.plan !== job.plan ||
          timeline.source !== job.source
        )
          throw new Error(
            "Timeline does not match the selected retained plan.",
          );
        if (abort.signal.aborted) return;
        active = !terminalBuild(timeline.state);
        setResult({ timeline, report, scope });
        setError("");
      } catch (error) {
        if (!abort.signal.aborted) setError(message(error));
      }
      if (!abort.signal.aborted && active)
        timer = setTimeout(() => {
          void read();
        }, 1500);
    }
    void read();
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
  }, [
    workspace,
    state,
    job.build,
    job.plan,
    job.source,
    job.build_state,
    revision,
  ]);
  return {
    result: result?.timeline.build === job.build ? result : undefined,
    error,
    refresh: () => refresh((n) => n + 1),
  };
}
export function useBuildClock(active: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return BigInt(now) * 1000n;
}
export function orderedJobs(jobs: readonly ApiJobTimingV1[]) {
  return [...jobs].sort((a, b) => {
    const x = a.attempts[0]?.started_us,
      y = b.attempts[0]?.started_us;
    if (x === undefined || y === undefined)
      return x === y ? a.job.localeCompare(b.job) : x === undefined ? 1 : -1;
    return BigInt(x) < BigInt(y)
      ? -1
      : BigInt(x) > BigInt(y)
        ? 1
        : a.job.localeCompare(b.job);
  });
}
