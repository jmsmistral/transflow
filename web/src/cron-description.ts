/** Read-only wording of the scheduler's bounded five-field numeric cron dialect.
 * Execution and timezone/DST resolution remain owned by the scheduler.
 */
type Field = { values: number[]; wildcard: boolean };
function field(
  text: string,
  low: number,
  high: number,
  sunday = false,
): Field | null {
  const values = new Set<number>();
  for (const part of text.split(",")) {
    if (!/^(\*|\d+(?:-\d+)?)(?:\/\d+)?$/.test(part)) return null;
    const [range = "", stepText] = part.split("/");
    const step = stepText === undefined ? 1 : Number(stepText);
    if (step < 1 || step > high - low + 1) return null;
    const [first = "", last] = range.split("-");
    const start = range === "*" ? low : Number(first);
    const end =
      range === "*"
        ? high
        : last !== undefined
          ? Number(last)
          : stepText !== undefined
            ? high
            : start;
    if (start < low || end > high || start > end) return null;
    for (let n = start; n <= end; n += step)
      values.add(sunday && n === 7 ? 0 : n);
  }
  return values.size
    ? { values: [...values].sort((a, b) => a - b), wildcard: text === "*" }
    : null;
}
const days = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
];
const months = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];
function list(values: readonly string[]): string {
  return values.length < 3
    ? values.join(" and ")
    : `${values.slice(0, -1).join(", ")} and ${values.at(-1) ?? ""}`;
}
function ordinal(n: number): string {
  return `${n}${n % 100 >= 11 && n % 100 <= 13 ? "th" : n % 10 === 1 ? "st" : n % 10 === 2 ? "nd" : n % 10 === 3 ? "rd" : "th"}`;
}
function time(hour: number, minute: number): string {
  return `${hour % 12 || 12}${minute ? `:${String(minute).padStart(2, "0")}` : ""}${hour < 12 ? "am" : "pm"}`;
}
function minutes(values: number[]): string {
  if (values.length === 1 && values[0] === 0) return "every hour";
  const step = values[1];
  if (
    values[0] === 0 &&
    step &&
    60 % step === 0 &&
    values.every((n, i) => n === i * step) &&
    values.length === Math.ceil(60 / step)
  )
    return step === 1 ? "every minute" : `every ${step} minutes`;
  return `at ${list(values.map(String))} ${values.length === 1 ? "minute" : "minutes"} past each hour`;
}
export function cronDescription(expression: string): string {
  const raw = expression.trim().split(/\s+/);
  if (expression.length > 256 || raw.length !== 5)
    return "Time condition needs review";
  const minute = field(raw[0] ?? "", 0, 59);
  const hour = field(raw[1] ?? "", 0, 23);
  const dom = field(raw[2] ?? "", 1, 31);
  const month = field(raw[3] ?? "", 1, 12);
  const dow = field(raw[4] ?? "", 0, 7, true);
  if (!minute || !hour || !dom || !month || !dow)
    return "Time condition needs review";
  const weekday =
    dow.values.join(",") === "1,2,3,4,5"
      ? "every weekday"
      : dow.values.join(",") === "0,6"
        ? "every weekend day"
        : `every ${list(dow.values.map((n) => days[n] ?? ""))}`;
  const dates = `on the ${list(dom.values.map(ordinal))} of each month`;
  // Restricted day-of-month and day-of-week fields use OR, including */1.
  let calendar =
    !dom.wildcard && !dow.wildcard
      ? dom.values.length === 31 || dow.values.length === 7
        ? "every day"
        : `${dates} or ${weekday}`
      : dom.wildcard
        ? dow.values.length === 7
          ? "every day"
          : weekday
        : dom.values.length === 31
          ? "every day"
          : dates;
  if (month.values.length !== 12)
    calendar = `in ${list(month.values.map((n) => months[n - 1] ?? ""))}, ${calendar}`;
  let clock: string;
  const hourStep = hour.values[1];
  if (
    minute.values.length === 1 &&
    hour.values[0] === 0 &&
    hourStep &&
    24 % hourStep === 0 &&
    hour.values.length === 24 / hourStep &&
    hour.values.every((n, i) => n === i * hourStep)
  )
    clock = `every ${hourStep === 1 ? "hour" : `${hourStep} hours`}${minute.values[0] ? ` at ${minute.values[0]} minutes past the hour` : ""}`;
  else if (hour.values.length === 24) clock = minutes(minute.values);
  else if (minute.values.length * hour.values.length <= 8)
    clock = `at ${list(hour.values.flatMap((h) => minute.values.map((m) => time(h, m))))}`;
  else
    clock = `${minutes(minute.values)} during the ${list(hour.values.map((h) => time(h, 0)))} ${hour.values.length === 1 ? "hour" : "hours"}`;
  const description =
    calendar === "every day" && clock.startsWith("every")
      ? clock
      : `${calendar}${clock.startsWith("every") ? ", " : " "}${clock}`;
  return description.charAt(0).toUpperCase() + description.slice(1);
}
