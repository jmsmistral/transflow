import type { BuildEvidence } from "./build-evidence";
import { field } from "./inspectors";
import { items, property } from "./execution-ui";

/** Resolve accepted target references only for display; never change build scope. */
export function buildCaption(
  evidence:
    | {
        report: BuildEvidence["report"];
        timeline: Pick<BuildEvidence["timeline"], "jobs">;
      }
    | undefined,
  openedPath: string,
): string {
  if (!evidence) return "Loading build…";
  const { report, timeline } = evidence;
  const trigger = property(report, "trigger");
  if (field(trigger, "kind") === "schedule") {
    const name = field(report, "schedule_name") ?? field(trigger, "name");
    return name
      ? `Build of schedule ${name}`
      : "Build of schedule (name unavailable)";
  }
  const plan = property(report, "plan");
  const writes = items(property(plan, "writes"));
  const refs = items(property(property(plan, "context"), "targets")).filter(
    (v): v is string => typeof v === "string",
  );
  const targets = refs.map((ref) => {
    const tail = ref.startsWith("dataset:") ? ref.split(":").at(-1) : ref;
    const id =
      tail && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(tail)
        ? tail.toLowerCase()
        : undefined;
    const job = timeline.jobs.find((j) => j.dataset === id || j.path === ref);
    const write = writes.find(
      (w) =>
        (id !== undefined && field(w, "dataset") === id) ||
        field(w, "path") === ref,
    );
    return {
      key: job?.dataset ?? field(write, "dataset") ?? id ?? ref,
      name: job?.path ?? field(write, "path") ?? (id ? undefined : ref),
    };
  });
  if (targets.length) {
    const count = new Set(targets.map((t) => t.key)).size;
    if (count > 1) return `Build of ${count} datasets`;
    return targets[0]?.name
      ? `Build of ${targets[0].name}`
      : "Build of 1 dataset";
  }
  if (timeline.jobs.length > 1)
    return `Build of ${timeline.jobs.length} datasets`;
  const job = timeline.jobs[0];
  const write = job && writes.find((w) => field(w, "dataset") === job.dataset);
  return `Build of ${job?.path ?? field(write, "path") ?? openedPath}`;
}
