import type {
  ApiLineageNodeV1,
  GraphViewV1,
  PlanResultV1,
  ApiOverlayFactsV1,
} from "../generated/contracts";
export type Colour = GraphViewV1["colour"];
export const colourOptions: ReadonlyArray<readonly [Colour, string]> = [
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
const names: Record<string, string> = {
  polars_transform: "Polars Transform",
  pandas_transform: "pandas Transform",
  sql_transform: "SQL Transform",
  source: "Source",
  imported: "Imported Dataset",
  external: "External Dataset",
  dataset: "Dataset",
  unknown: "Unknown",
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
  planned: "Planned job",
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
      ? [1024n ** 2n, 1024n ** 3n, 1024n ** 4n]
      : dimension.startsWith("duration")
        ? [60_000_000_000n, 300_000_000_000n, 900_000_000_000n]
        : dimension === "files"
          ? [10n, 100n, 1000n]
          : [1000n, 1_000_000n, 1_000_000_000n];
  const labels =
    dimension === "bytes"
      ? ["< 1 MiB", "1 MiB – < 1 GiB", "1 GiB – < 1 TiB", "≥ 1 TiB"]
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
      ? `${n.toLocaleString()} B`
      : dimension.startsWith("duration")
        ? `${(n / 1_000_000n).toLocaleString()} ms`
        : `${n.toLocaleString()} ${dimension === "files" ? "files" : "rows"}`;
  return {
    key: n === 0n ? "zero" : `bin${bin < 0 ? 3 : bin}`,
    label:
      n === 0n
        ? dimension === "bytes"
          ? "0 B"
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
export function paint(
  node: ApiLineageNodeV1,
  colour: Colour,
  plan: PlanResultV1 | null = null,
): Paint {
  const f = node.overlay;
  const badges = [
    ...(f?.version && colour === "status" ? ["Published data available"] : []),
    ...(f?.input_failed ? ["Input check failed"] : []),
    ...(colour === "roles" ? roles(node, plan).map((r) => names[r] ?? r) : []),
  ];
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
          ? (f?.latest_attempt ?? "unknown")
          : colour === "health"
            ? (f?.health ?? "Unavailable")
            : colour === "freshness"
              ? freshness(node, plan)
              : ([
                  "excluded",
                  "target",
                  "planned",
                  "trigger",
                  "boundary",
                  "cached",
                ].find((r) => roles(node, plan).includes(r)) ?? "unknown");
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
      colour === "freshness" && reasons.length
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
                ? "Active build preview. Schedule roles unavailable until scheduling is enabled."
                : colour === "freshness"
                  ? "Shared freshness evidence. Unverified bytes or policies remain unknown."
                  : "Visible nodes only.";
}
