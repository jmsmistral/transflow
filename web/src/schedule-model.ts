import type {
  DatasetKey,
  ScheduleDefinitionV1,
  ScheduleTriggerV1,
  ApiDatasetV1,
  ScheduleRecordV1,
  ApiLineageNodeV1,
} from "./generated/contracts";
import { decode } from "./api/client";
export const identity = (key: DatasetKey) =>
  `dataset:${key.workspace_id}:${key.dataset_id}`;
export const keyOf = (d: ApiDatasetV1): DatasetKey => ({
  workspace_id: d.workspace_id,
  dataset_id: d.dataset_id,
});
export type ScheduleGraphAction = "target" | "trigger" | "clear";
export type ScheduleEditSession = {
  definition: ScheduleDefinitionV1;
  busy: boolean;
  act: (
    action: ScheduleGraphAction,
    nodes: readonly ApiLineageNodeV1[],
  ) => void;
  exit: () => void;
};
export function graphKey(node: ApiLineageNodeV1): DatasetKey | null {
  const [kind, workspace_id, dataset_id, extra] = node.identity.split(":");
  return kind === "dataset" && workspace_id && dataset_id && !extra
    ? { workspace_id, dataset_id }
    : null;
}
export function editGraphRoles(
  definition: ScheduleDefinitionV1,
  action: ScheduleGraphAction,
  nodes: readonly ApiLineageNodeV1[],
  newId = () => `condition-${crypto.randomUUID()}`,
): ScheduleDefinitionV1 {
  const keys = nodes.flatMap((n) => {
    const key = graphKey(n);
    return key && (action !== "target" || (!n.external && n.producer))
      ? [key]
      : [];
  });
  const ids = new Set(keys.map(identity));
  const remove = (values: readonly DatasetKey[]) =>
    values.filter((k) => !ids.has(identity(k)));
  if (action === "clear") {
    const prune = (t: ScheduleTriggerV1): ScheduleTriggerV1 | null => {
      if (t.kind === "and" || t.kind === "or") {
        const children = t.children.flatMap((c) => {
          const next = prune(c);
          return next ? [next] : [];
        });
        return children.length > 1 ? { ...t, children } : (children[0] ?? null);
      }
      if (t.kind === "dataset_published" || t.kind === "dataset_head_changed")
        return ids.has(identity(t.dataset)) ? null : t;
      if (t.kind === "build_succeeded") {
        const targets = remove(t.targets);
        return targets.length ? { ...t, targets } : null;
      }
      return t;
    };
    return {
      ...definition,
      trigger: prune(definition.trigger) ?? { kind: "manual" },
      build: {
        ...definition.build,
        targets: remove(definition.build.targets),
        boundaries: remove(definition.build.boundaries),
        exclusions: remove(definition.build.exclusions),
        refresh_sources: remove(definition.build.refresh_sources),
      },
    };
  }
  if (action === "target") {
    const targets = new Map(
      definition.build.targets.map((k) => [identity(k), k]),
    );
    keys.forEach((k) => targets.set(identity(k), k));
    return {
      ...definition,
      build: { ...definition.build, targets: [...targets.values()] },
    };
  }
  const existing = new Set(triggerDatasets(definition.trigger).map(identity));
  const added: ScheduleTriggerV1[] = [];
  for (const key of keys)
    if (!existing.has(identity(key))) {
      existing.add(identity(key));
      added.push({
        kind: "dataset_published",
        id: newId(),
        dataset: key,
        branch: definition.build.data_branch,
        payload_mode: "pin",
        include_resets: false,
      });
    }
  const first = added[0];
  if (!first) return definition;
  const trigger = definition.trigger;
  return {
    ...definition,
    trigger:
      trigger.kind === "manual"
        ? added.length === 1
          ? first
          : { kind: "and", children: added }
        : trigger.kind === "and" || trigger.kind === "or"
          ? { ...trigger, children: [...trigger.children, ...added] }
          : { kind: "and", children: [trigger, ...added] },
  };
}
export function secondsLabel(seconds: number): string {
  const hours = Math.floor(seconds / 3600),
    minutes = Math.floor((seconds % 3600) / 60),
    rest = seconds % 60;
  return [
    hours ? `${hours} h` : "",
    minutes ? `${minutes} min` : "",
    rest || (!hours && !minutes) ? `${rest} s` : "",
  ]
    .filter(Boolean)
    .join(" ");
}
export function savedDefinition(
  row: ScheduleRecordV1,
): ScheduleDefinitionV1 | null {
  try {
    return decode("ScheduleDefinitionV1", row.definition);
  } catch {
    return null;
  }
}
export function triggerDatasets(
  trigger: ScheduleTriggerV1,
): readonly DatasetKey[] {
  switch (trigger.kind) {
    case "and":
    case "or":
      return trigger.children.flatMap(triggerDatasets);
    case "dataset_published":
    case "dataset_head_changed":
      return [trigger.dataset];
    case "build_succeeded":
      return trigger.targets;
    default:
      return [];
  }
}
export function related(
  definition: ScheduleDefinitionV1,
  nodes: readonly string[],
) {
  const targets = definition.build.targets.some((d) =>
    nodes.includes(identity(d)),
  );
  const triggers = triggerDatasets(definition.trigger).some((d) =>
    nodes.includes(identity(d)),
  );
  return {
    targets,
    triggers,
    any:
      targets ||
      triggers ||
      definition.build.boundaries.some((d) => nodes.includes(identity(d))),
  };
}
export function minute(us: string | null): string {
  if (us == null) return "—";
  const n = BigInt(us) / 1000n;
  if (n > 8_640_000_000_000_000n) return "Outside display range";
  return new Date(Number(n)).toISOString().slice(0, 16).replace("T", "  ");
}
export function duration(us: string | null): string {
  if (us == null) return "—";
  const n = BigInt(us),
    hundredths = (n + 5000n) / 10000n;
  return `${hundredths / 100n}.${(hundredths % 100n).toString().padStart(2, "0")} s`;
}
export function ratio(
  value: { readonly numerator: string; readonly denominator: string } | null,
): string {
  if (!value) return "—";
  return duration(
    (BigInt(value.numerator) / BigInt(value.denominator)).toString(),
  );
}
