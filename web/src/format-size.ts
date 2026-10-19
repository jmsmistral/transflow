/** Decimal units; keep large retained counts exact until display rounding. */
export function formatSize(value: unknown): string {
  if (typeof value !== "string" || !/^\d+$/.test(value)) return "Unavailable";
  const bytes = BigInt(value);
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let divisor = 1000n,
    unit = 0;
  while (bytes >= divisor * 1000n && unit < units.length - 1) {
    divisor *= 1000n;
    unit++;
  }
  const hundredths = (bytes * 100n + divisor / 2n) / divisor;
  const fraction = (hundredths % 100n)
    .toString()
    .padStart(2, "0")
    .replace(/0+$/, "");
  return `${hundredths / 100n}${fraction ? `.${fraction}` : ""} ${units[unit]}`;
}
