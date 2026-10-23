import { formatSize } from "../format-size";
import type {
  ApiLineageNodeV1,
  GraphViewV1,
  PlanResultV1,
  ApiOverlayFactsV1,
} from "../generated/contracts";
export type Colour = GraphViewV1["colour"];
const colourLabels: ReadonlyArray<readonly [Colour, string]> = [
  ["resource", "Resource Type"],
  ["publication", "Publication"],
  ["status", "Build status"],
  ["freshness", "Out of date"],
  ["duration", "Build duration"],
  ["duration_median", "Median duration"],
  ["duration_mean", "Mean duration"],
  ["rows", "Row count"],
  ["files", "File count"],
  ["bytes", "Dataset size"],
  ["health", "Data health"],
  ["roles", "Build roles"],
];
// Retain labels for saved views and automatic schedule roles, while keeping
// the user-facing menu focused on the supported colouring choices.
export const colourOptions = colourLabels.filter(
  ([key]) => !["duration_median", "duration_mean", "roles"].includes(key),
);
export const colourLabel = (colour: Colour) =>
  colourLabels.find(([key]) => key === colour)?.[1] ?? "Resource Type";
const names: Record<string, string> = {
  polars_transform: "Polars Transform",
  sql_transform: "SQL Transform",
  source: "Source",
  imported: "Imported Dataset",
  external: "External Dataset",
  dataset: "Dataset",
  unknown: "Unknown",
  not_built: "Not built",
  no_build: "No build on branch",
  published: "Published",
  missing: "Not built on these branches",
  Running: "Running",
  Succeeded: "Succeeded",
  Failed: "Failed",
  Canceled: "Canceled",
  Interrupted: "Interrupted",
  current: "Current",
  data: "Data changed",
  logic: "Logic changed",
  ancestor: "Ancestor changed",
  never_built: "Never built",
  Passed: "Passed",
  Warning: "Warning",
  NoChecks: "No health checks",
  Unavailable: "Unavailable",
  excluded: "Excluded",
  target: "Target",
  planned: "Attempt to build",
  trigger: "Trigger",
  boundary: "Read boundary",
  cached: "Cached",
};
export interface Paint {
  key: string;
  label: string;
  detail: string;
  badges: readonly string[];
}
export function roles(
  node: ApiLineageNodeV1,
  plan: PlanResultV1 | null,
): string[] {
  const recorded = node.overlay?.roles ?? [];
  const matches = (paths: readonly string[]) =>
    node.paths.some((p) => paths.includes(p)) || paths.includes(node.identity);
  return [
    ...new Set([
      ...recorded,
      ...(plan && matches(plan.exclusions) ? ["excluded"] : []),
      ...(plan && matches(plan.targets) ? ["target"] : []),
      ...(plan?.writes.some((w) => node.paths.includes(w.path))
        ? ["planned"]
        : []),
      ...(plan?.reads.some((r) => node.paths.includes(r.path))
        ? ["boundary"]
        : []),
    ]),
  ];
}
/** Schedule roles describe intent; input/boundary/cache facts remain planning details. */
export function schedulePaint(
  node: ApiLineageNodeV1,
  plan: PlanResultV1 | null,
  connecting: boolean,
): Paint {
  const badges = node.overlay?.input_failed ? ["Input check failed"] : [];
  if (node.overlay?.roles === undefined && !plan)
    return { key: "unknown", label: "Unknown", detail: "Unknown", badges };
  const all = roles(node, plan);
  const flags: string[] = all.filter(
    (role) => role === "target" || role === "trigger",
  );
  if (!flags.length && connecting && all.includes("planned"))
    flags.push("planned");
  const key =
    ["target", "trigger", "planned"].find((role) => flags.includes(role)) ??
    "not_built";
  return {
    key,
    label: names[key] ?? "Not built",
    detail: flags.map((role) => names[role]).join(" · ") || "Not built",
    badges,
  };
}
function freshness(node: ApiLineageNodeV1, plan: PlanResultV1 | null): string {
  const f = plan?.freshness.find((f) => f.identity === node.identity);
  if (!f) return node.overlay?.freshness ?? "unknown";
  return f.materialization === "NeverBuilt"
    ? "never_built"
    : f.direct_data === "Stale"
      ? "data"
      : f.direct_logic === "Stale"
        ? "logic"
        : f.inherited === "Stale"
          ? "ancestor"
          : [f.direct_data, f.direct_logic, f.inherited].every(
                (x) => x === "Current",
              )
            ? "current"
            : "unknown";
}
/** Half-open fixed bins. Zero and unknown are separate; Count never passes through Number. */
export function numeric(
  value: string | null | undefined,
  dimension: Colour,
): Paint {
  if (value == null)
    return { key: "unknown", label: "Unknown", detail: "Unknown", badges: [] };
  const n = BigInt(value);
  const limits =
    dimension === "bytes"
      ? [1000n ** 2n, 1000n ** 3n, 1000n ** 4n]
      : dimension.startsWith("duration")
        ? [60_000_000_000n, 300_000_000_000n, 900_000_000_000n]
        : dimension === "files"
          ? [10n, 100n, 1000n]
          : [1000n, 1_000_000n, 1_000_000_000n];
  const labels =
    dimension === "bytes"
      ? ["< 1 MB", "1 MB - < 1 GB", "1 GB - < 1 TB", "≥ 1 TB"]
      : dimension.startsWith("duration")
        ? ["< 1 min", "1 – < 5 min", "5 – < 15 min", "≥ 15 min"]
        : dimension === "files"
          ? [
              "1 – < 10 files",
              "10 – < 100 files",
              "100 – < 1,000 files",
              "≥ 1,000 files",
            ]
          : [
              "1 – < 1,000 rows",
              "1,000 – < 1M rows",
              "1M – < 1B rows",
              "≥ 1B rows",
            ];
  const bin = limits.findIndex((limit) => n < limit);
  const detail =
    dimension === "bytes"
      ? formatSize(value)
      : dimension.startsWith("duration")
        ? `${(n / 1_000_000n).toLocaleString()} ms`
        : `${n.toLocaleString()} ${dimension === "files" ? "files" : "rows"}`;
  return {
    key: n === 0n ? "zero" : `bin${bin < 0 ? 3 : bin}`,
    label:
      n === 0n
        ? dimension === "bytes"
          ? "0 KB"
          : dimension.startsWith("duration")
            ? "0 s"
            : "Zero"
        : (labels[bin < 0 ? 3 : bin] ?? "Unknown"),
    detail,
    badges: [],
  };
}
function duration(
  facts: ApiOverlayFactsV1 | undefined,
  colour: Colour,
): string | null {
  const d = facts?.durations;
  if (!d) return null;
  if (colour === "duration") return d.last_ns;
  const ratio = colour === "duration_median" ? d.median_ns : d.mean_ns;
  return ratio && BigInt(ratio.denominator) > 0n
    ? (BigInt(ratio.numerator) / BigInt(ratio.denominator)).toString()
    : null;
}
function buildStatus(node: ApiLineageNodeV1): string {
  const attempt = node.overlay?.latest_attempt;
  if (attempt != null) return attempt;
  // Explicit absence in local branch facts differs from unavailable metadata.
  return attempt === null && !node.external && node.publication !== "unknown"
    ? "no_build"
    : "unknown";
}
export function paint(
  node: ApiLineageNodeV1,
  colour: Colour,
  plan: PlanResultV1 | null = null,
): Paint {
  const f = node.overlay;
  const badges = [...(f?.input_failed ? ["Input check failed"] : [])];
  if (
    ["rows", "files", "bytes"].includes(colour) ||
    colour.startsWith("duration")
  ) {
    const p = numeric(
      colour.startsWith("duration")
        ? duration(f, colour)
        : colour === "rows"
          ? f?.rows
          : colour === "files"
            ? f?.files
            : f?.bytes,
      colour,
    );
    return { ...p, badges };
  }
  const key =
    colour === "resource"
      ? node.resource_type
      : colour === "publication"
        ? node.publication
        : colour === "status"
          ? buildStatus(node)
          : colour === "health"
            ? (f?.health ?? "Unavailable")
            : colour === "freshness"
              ? freshness(node, plan)
              : ([
                  "excluded",
                  "target",
                  "trigger",
                  "planned",
                  "boundary",
                  "cached",
                ].find((r) => roles(node, plan).includes(r)) ??
                (f?.roles !== undefined || plan ? "not_built" : "unknown"));
  const reasons =
    plan?.freshness
      .find((s) => s.identity === node.identity)
      ?.reasons.map((r) => r.message) ??
    f?.reasons ??
    [];
  return {
    key,
    label: names[key] ?? "Unknown",
    detail:
      colour === "roles"
        ? roles(node, plan)
            .map((role) => names[role] ?? role)
            .join(" · ") ||
          (names[key] ?? "Unknown")
        : colour === "freshness" && reasons.length
          ? reasons.join("\n")
          : (names[key] ?? "Unknown"),
    badges,
  };
}
export function basis(colour: Colour): string {
  return colour === "duration"
    ? "Latest successful materialization. Cache reuse excluded."
    : colour === "duration_median"
      ? "Median of measured durations in the last 10 successful materializations."
      : colour === "duration_mean"
        ? "Mean of the last 10 successful materializations; unknown if a timing is missing."
        : ["rows", "files", "bytes"].includes(colour)
          ? "Committed metadata. Zero is separate from unknown."
          : colour === "status"
            ? "Latest actual attempt; published data is shown separately."
            : colour === "health"
              ? "Published output checks; input failures are separate badges."
              : colour === "roles"
                ? "Saved schedule roles and active scope preview. Hover the role label for simultaneous roles."
                : colour === "freshness"
                  ? "Shared freshness evidence. Unverified bytes or policies remain unknown."
                  : "Visible nodes only.";
}
