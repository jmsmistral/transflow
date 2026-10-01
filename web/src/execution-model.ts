import type { ApiExactRatioV1, ApiHistoryJobV1 } from "./generated/contracts";

/** Inclusive calendar dates map to the API's half-open UTC microsecond range. */
export function historyDateRange(from: string, to: string) {
  const parse = (value: string): number | null => {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
    const ms = Date.parse(`${value}T00:00:00Z`);
    return Number.isFinite(ms) &&
      ms >= 0 &&
      new Date(ms).toISOString().slice(0, 10) === value
      ? ms
      : null;
  };
  const start = parse(from),
    end = parse(to);
  if (start === null || end === null || end < start) return null;
  return {
    from_us: (BigInt(start) * 1000n).toString(),
    to_us: (BigInt(end + 86400000) * 1000n).toString(),
  };
}

export function historyDate(us: string, inclusiveEnd = false): string {
  const ms = BigInt(us) / 1000n - (inclusiveEnd ? 86400000n : 0n);
  if (ms > 8640000000000000n || ms < -8640000000000000n)
    return "Outside calendar range";
  return new Date(Number(ms)).toISOString().slice(0, 10);
}

/** Display rounding uses integer arithmetic; retained nanoseconds remain available in titles. */
export function seconds(
  value: string | ApiExactRatioV1 | null | undefined,
): string {
  if (value == null) return "Unavailable";
  const n = BigInt(typeof value === "string" ? value : value.numerator);
  const d = typeof value === "string" ? 1n : BigInt(value.denominator);
  if (d <= 0n) return "Unavailable";
  const ms = (n + d * 500000n) / (d * 1000000n);
  return `${ms / 1000n}.${(ms % 1000n).toString().padStart(3, "0")} s`;
}
export function percent(value: ApiExactRatioV1 | null): string {
  if (!value || BigInt(value.denominator) === 0n) return "Unavailable";
  const n = (BigInt(value.numerator) * 10000n) / BigInt(value.denominator);
  return `${n / 100n}.${(n % 100n).toString().padStart(2, "0")}%`;
}
export function timestamp(us: string | null | undefined): string {
  if (us == null) return "Unavailable";
  const milliseconds = BigInt(us) / 1000n;
  if (milliseconds > 8640000000000000n || milliseconds < -8640000000000000n)
    return "Outside calendar range";
  return (
    new Date(Number(milliseconds))
      .toISOString()
      .replace("T", " ")
      .slice(0, 19) + " UTC"
  );
}
export function short(id: string | null | undefined): string {
  return id?.slice(0, 8) ?? "Unavailable";
}
export function sortedHistory(
  jobs: readonly ApiHistoryJobV1[],
): ApiHistoryJobV1[] {
  return [...jobs].sort((a, b) =>
    BigInt(a.created_us) > BigInt(b.created_us)
      ? -1
      : BigInt(a.created_us) < BigInt(b.created_us)
        ? 1
        : b.id.localeCompare(a.id),
  );
}
/** Normalize only bounded drawing coordinates, never counts or measured values. */
export function position(value: bigint, start: bigint, span: bigint): number {
  if (span <= 0n) return 0;
  return Number(((value - start) * 10000n) / span) / 100;
}
export function sourceDiff(
  before: string,
  after: string,
): readonly { kind: "same" | "removed" | "added"; text: string }[] {
  const a = before.split("\n"),
    b = after.split("\n");
  let first = 0,
    last = 0;
  while (first < a.length && first < b.length && a[first] === b[first]) first++;
  while (
    last < a.length - first &&
    last < b.length - first &&
    a[a.length - last - 1] === b[b.length - last - 1]
  )
    last++;
  return [
    ...a.slice(0, first).map((text) => ({ kind: "same" as const, text })),
    ...a
      .slice(first, a.length - last)
      .map((text) => ({ kind: "removed" as const, text })),
    ...b
      .slice(first, b.length - last)
      .map((text) => ({ kind: "added" as const, text })),
    ...a
      .slice(a.length - last)
      .map((text) => ({ kind: "same" as const, text })),
  ];
}
