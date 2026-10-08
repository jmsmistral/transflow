import type {
  DatasetKey,
  ScheduleDefinitionV1,
  ScheduleTriggerV1,
  ApiDatasetV1,
  ScheduleRecordV1,
} from "./generated/contracts";
import { decode } from "./api/client";
export const identity = (key: DatasetKey) =>
  `dataset:${key.workspace_id}:${key.dataset_id}`;
export const keyOf = (d: ApiDatasetV1): DatasetKey => ({
  workspace_id: d.workspace_id,
  dataset_id: d.dataset_id,
});
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
