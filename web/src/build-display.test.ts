import { expect, test } from "vitest";
import { buildCaption } from "./build-display";
import type { ApiJobTimingV1, ExecutionJsonV1 } from "./generated/contracts";

const ids = [
  "00000000-0000-4000-8000-000000000001",
  "00000000-0000-4000-8000-000000000002",
  "00000000-0000-4000-8000-000000000003",
  "00000000-0000-4000-8000-000000000004",
] as const;
const jobs: ApiJobTimingV1[] = ids.map((id, i) => ({
  job: id,
  dataset: id,
  path: `curated/item${i + 1}`,
  state: "SUCCEEDED",
  duration_ns: "1",
  attempts: [],
}));
const caption = (
  targets: readonly string[],
  trigger: ExecutionJsonV1 = { kind: "manual" },
  extra: Readonly<Record<string, ExecutionJsonV1>> = {},
) =>
  buildCaption(
    {
      report: { trigger, plan: { context: { targets } }, ...extra },
      timeline: { jobs },
    },
    "opened/node",
  );

test("manual captions count accepted targets and resolve dataset references to paths", () => {
  expect(caption(ids.map((id) => `dataset:${id}`))).toBe("Build of 4 datasets");
  expect(caption([`dataset:${ids[0]}`])).toBe("Build of curated/item1");
  expect(
    caption([`dataset:00000000-0000-4000-8000-000000000099:${ids[1]}`]),
  ).toBe("Build of curated/item2");
  expect(caption([ids[0]])).toBe("Build of curated/item1");
  expect(caption(["curated/item3"])).toBe("Build of curated/item3");
  expect(caption([`dataset:${ids[0]}`, "curated/item1"])).toBe(
    "Build of curated/item1",
  );
  expect(caption(["dataset:00000000-0000-4000-8000-000000000099"])).toBe(
    "Build of 1 dataset",
  );
});
test("scheduled captions prefer resolved names and never print the schedule UUID as a name", () => {
  expect(
    caption(
      ids,
      { kind: "schedule", schedule_id: ids[0] },
      { schedule_name: "Daily items" },
    ),
  ).toBe("Build of schedule Daily items");
  expect(caption(ids, { kind: "schedule", name: "Captured name" })).toBe(
    "Build of schedule Captured name",
  );
  expect(caption(ids, { kind: "schedule", schedule_id: ids[0] })).toBe(
    "Build of schedule (name unavailable)",
  );
});
test("older reports resolve write paths without labelling other jobs as requested targets", () => {
  expect(
    caption(
      [`dataset:${ids[0]}`],
      { kind: "manual" },
      {
        plan: {
          context: { targets: [`dataset:${ids[0]}`] },
          writes: [{ dataset: ids[0], path: "retained/path" }],
        },
      },
    ),
  ).toBe("Build of curated/item1");
  expect(
    buildCaption(
      {
        report: {
          plan: {
            context: { targets: [`dataset:${ids[0]}`] },
            writes: [{ dataset: ids[0], path: "retained/path" }],
          },
        },
        timeline: { jobs: [] },
      },
      "opened/node",
    ),
  ).toBe("Build of retained/path");
  expect(buildCaption({ report: {}, timeline: { jobs } }, "opened/node")).toBe(
    "Build of 4 datasets",
  );
  expect(
    buildCaption({ report: {}, timeline: { jobs: [] } }, "opened/node"),
  ).toBe("Build of opened/node");
  expect(buildCaption(undefined, "opened/node")).toBe("Loading build…");
});
