/** Explicit local calendar date and time, independent of the browser's date ordering. */
export function formatTimestamp(microseconds: unknown): string {
  if (typeof microseconds !== "string" || !/^-?\d+$/u.test(microseconds))
    return "Not available";
  const date = new Date(Number(BigInt(microseconds) / 1000n));
  if (!Number.isFinite(date.getTime())) return "Not available";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${String(date.getFullYear()).padStart(4, "0")}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}
