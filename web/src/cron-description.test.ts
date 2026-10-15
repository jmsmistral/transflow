import { expect, test } from "vitest";
import { cronDescription } from "./cron-description";

test.each([
  ["0 13 * * 3", "Every Wednesday at 1pm"],
  ["0 9 * * 1-5", "Every weekday at 9am"],
  ["15 0 * * *", "Every day at 12:15am"],
  ["0 12 * * *", "Every day at 12pm"],
  ["0 9 * * 0,7", "Every Sunday at 9am"],
  ["0 9 * * 6,0", "Every weekend day at 9am"],
  ["*/15 * * * *", "Every 15 minutes"],
  [
    "*/7 * * * *",
    "Every day at 0, 7, 14, 21, 28, 35, 42, 49 and 56 minutes past each hour",
  ],
  ["* * * * *", "Every minute"],
  ["0 * * * *", "Every hour"],
  ["0 */2 * * *", "Every 2 hours"],
  ["5,35 * * * *", "Every day at 5 and 35 minutes past each hour"],
  ["0 9,17 * * 1,3,5", "Every Monday, Wednesday and Friday at 9am and 5pm"],
  ["0 9 1,15 * *", "On the 1st and 15th of each month at 9am"],
  ["0 9 11-13 * *", "On the 11th, 12th and 13th of each month at 9am"],
  ["0 9 1 * 1", "On the 1st of each month or every Monday at 9am"],
  ["0 9 */1 * 1", "Every day at 9am"],
  [
    "0 9 1 */2 *",
    "In January, March, May, July, September and November, on the 1st of each month at 9am",
  ],
])("describes %s without changing calendar meaning", (expression, expected) => {
  expect(cronDescription(expression)).toBe(expected);
});
test.each([
  "",
  "0 9 * *",
  "0 9 * * * *",
  "60 9 * * *",
  "0 24 * * *",
  "*/0 * * * *",
  "0 9 * * MON",
  "0 9 32 * *",
  "0 9 * 13 *",
  "0 9 * * 8",
  "0 9 3-1 * *",
])("does not invent a description for invalid %s", (expression) => {
  expect(cronDescription(expression)).toBe("Time condition needs review");
});
