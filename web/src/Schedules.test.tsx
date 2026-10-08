import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import { Schedules } from "./Schedules";
import { ScheduleEditor } from "./ScheduleEditor";
import {
  context,
  workspaceId,
  datasetId,
  fixtureTransport,
  json,
} from "./test-fixtures";
import type {
  ScheduleDefinitionV1,
  ScheduleRecordV1,
  ApiScheduleRolesV1,
} from "./generated/contracts";
import {
  related,
  savedDefinition,
  identity,
  duration,
  minute,
} from "./schedule-model";
const target = { workspace_id: workspaceId, dataset_id: datasetId };
const id = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const definition: ScheduleDefinitionV1 = {
  format_version: 1,
  name: "Daily review",
  description: "",
  trigger: {
    kind: "dataset_published",
    id: "published",
    dataset: target,
    branch: "input",
    payload_mode: "signal_only",
    include_resets: false,
  },
  build: {
    source: { kind: "fixed_snapshot", snapshot_id: workspaceId },
    data_branch: "output",
    fallback_branches: ["stable"],
    input_fallback_policy: { default: ["stable"], rules: {} },
    provider_fallback_policies: {},
    targets: [target],
    build_mode: "selected",
    boundaries: [],
    exclusions: [],
    refresh_sources: [],
    parameters: {},
    force: false,
    require_current: false,
    timeout_seconds: 3600,
    validation_timeout_seconds: 3600,
  },
  policies: {
    max_attempts: 1,
    retryable_classes: [],
    abort_on_failure: true,
    overlap_policy: "coalesce_latest",
    max_pending: 100,
    allow_overlapping_builds: false,
    misfire_policy: "coalesce_latest",
    max_catch_up: 10,
    token_window_seconds: 86400,
    acknowledge_no_expiry: false,
    max_consecutive_builds: 5,
    minimum_delay_seconds: 1,
  },
};
const record: ScheduleRecordV1 = {
  id,
  etag: "e".repeat(64),
  trigger_epoch: "f".repeat(64),
  paused: true,
  needs_review: false,
  definition,
  saved_at_us: "1",
};
const roles: ApiScheduleRolesV1 = {
  schedule_id: id,
  etag: record.etag,
  basis: "saved_definition",
  total: "1",
  next_cursor: null,
  nodes: [
    {
      identity: identity(target),
      paths: ["raw/example"],
      roles: ["target", "trigger"],
    },
  ],
};
async function setup(conflict = false) {
  const mutations: { method: string; headers: Headers; body: unknown }[] = [];
  const base = fixtureTransport((path) => {
    if (path === "/api/v1/schedules")
      return Promise.resolve(
        json({ schedules: [record], next_cursor: null }, null),
      );
    if (path.endsWith("/roles")) return Promise.resolve(json(roles, context()));
    if (path.endsWith("/history"))
      return Promise.resolve(
        json({ schedule_id: id, occurrences: [], next_cursor: null }, null),
      );
    if (path.endsWith("/metrics"))
      return Promise.resolve(
        new Response(
          JSON.stringify({
            error: { message: "No measured history in this fixture" },
          }),
          { status: 409 },
        ),
      );
    if (path.endsWith("/defaults"))
      return Promise.resolve(json(definition, context()));
    return undefined;
  });
  const transport: typeof fetch = async (url, init) => {
    if (String(url) === "/api/v1/schedules/clock-preview")
      return json({ leaves: [] }, null);
    if (String(url) === `/api/v1/schedules/${id}`) {
      mutations.push({
        method: init?.method ?? "",
        headers: new Headers(init?.headers),
        body: JSON.parse(String(init?.body)) as unknown,
      });
      return conflict
        ? new Response(
            JSON.stringify({
              error: {
                message:
                  "The schedule changed; reload its current definition before saving",
              },
            }),
            { status: 409 },
          )
        : json(
            {
              ...record,
              definition: JSON.parse(String(init?.body)) as unknown,
            },
            null,
          );
    }
    return base(url, init);
  };
  const workspace = new Workspace(new Client(transport));
  await workspace.connect("test");
  const state = workspace.snapshot();
  if (state.kind !== "ready") throw Error("setup");
  return { workspace, state, mutations };
}
test("schedule membership retains target and trigger simultaneously and uses explicit branch identities", () => {
  expect(related(definition, [identity(target)])).toEqual({
    targets: true,
    triggers: true,
    any: true,
  });
  expect(related(definition, [`dataset:other:${datasetId}`]).any).toBe(false);
  expect(savedDefinition({ ...record, definition: { legacy: true } })).toBe(
    null,
  );
  expect(duration("2563933")).toBe("2.56 s");
  expect(minute("1791372225000000")).not.toContain(":25");
});
test("visible-graph lists split many-to-many membership; saved backend roles include hidden datasets", async () => {
  const { workspace, state } = await setup();
  const onRoles = vi.fn(),
    onPlan = vi.fn();
  const node = {
    identity: identity(target),
    paths: ["raw/example"],
    depth: "0",
    external: false,
    producer: true,
    parent_count: "0",
    child_count: "0",
    publication: "published",
    resource_type: "polars_transform",
  } as const;
  render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[node]}
      visible={[]}
      onRoles={onRoles}
      onPlan={onPlan}
      onAdd={vi.fn()}
    />,
  );
  await screen.findByText("Builds selected datasets");
  await waitFor(() =>
    expect(screen.getAllByText("Daily review").length).toBe(2),
  );
  const first = screen.getAllByText("Daily review")[0];
  if (!first) throw Error("schedule row");
  fireEvent.click(first);
  await waitFor(() => expect(onRoles).toHaveBeenCalledWith(roles));
  expect(
    screen.getByText(/1 relevant datasets.*0 visible.*1 hidden/),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", {
      name: "Add hidden relevant datasets to view",
    }),
  ).toBeTruthy();
  workspace.dispose();
});
test("cancel leaves the saved snapshot intact; save uses PUT and the original ETag and preserves source and branch settings", async () => {
  const { workspace, state, mutations } = await setup();
  const save = vi.fn(async (d: ScheduleDefinitionV1, signal: AbortSignal) => {
      await workspace.client.saveSchedule(d, record, signal);
    }),
    cancel = vi.fn();
  render(
    <ScheduleEditor
      workspace={workspace}
      initial={definition}
      record={record}
      datasets={state.value.datasets.entries}
      schedules={[record]}
      onSave={save}
      onCancel={cancel}
      onDefaults={async () => definition}
    />,
  );
  fireEvent.change(screen.getByLabelText("Schedule name"), {
    target: { value: "Edited" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(cancel).toHaveBeenCalled();
  expect(mutations.length).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  await waitFor(() => expect(mutations.length).toBe(1));
  expect(mutations[0]?.method).toBe("PUT");
  expect(mutations[0]?.headers.get("If-Match")).toBe(`"${record.etag}"`);
  expect(mutations[0]?.body).toMatchObject({
    name: "Edited",
    build: {
      source: definition.build.source,
      data_branch: "output",
      fallback_branches: ["stable"],
    },
  });
  workspace.dispose();
});
test("edit conflicts preserve the draft and explain which saved definition must be reloaded", async () => {
  const { workspace, state } = await setup(true);
  render(
    <ScheduleEditor
      workspace={workspace}
      initial={definition}
      record={record}
      datasets={state.value.datasets.entries}
      schedules={[record]}
      onSave={async (d, signal) => {
        await workspace.client.saveSchedule(d, record, signal);
      }}
      onCancel={vi.fn()}
      onDefaults={async () => definition}
    />,
  );
  fireEvent.change(screen.getByLabelText("Schedule name"), {
    target: { value: "Unsaved name" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    expect.stringContaining("reload its current definition"),
  );
  expect(screen.getByDisplayValue("Unsaved name")).toBeTruthy();
  workspace.dispose();
});
test("compound editor emits typed time trees and policy switches remain accessible", async () => {
  const { workspace, state } = await setup();
  const save = vi
    .fn<
      (definition: ScheduleDefinitionV1, signal: AbortSignal) => Promise<void>
    >()
    .mockResolvedValue(undefined);
  render(
    <ScheduleEditor
      workspace={workspace}
      initial={definition}
      record={record}
      datasets={state.value.datasets.entries}
      schedules={[record]}
      onSave={save}
      onCancel={vi.fn()}
      onDefaults={async () => definition}
    />,
  );
  fireEvent.change(screen.getByLabelText("Condition type"), {
    target: { value: "and" },
  });
  expect(screen.getAllByLabelText("Cron expression").length).toBe(2);
  const timezone = screen.getAllByLabelText("Timezone")[0];
  if (!timezone) throw Error("timezone control");
  fireEvent.change(timezone, {
    target: { value: "Asia/Dubai" },
  });
  fireEvent.click(screen.getByText("Advanced options"));
  fireEvent.click(
    screen.getByRole("switch", { name: "Allow overlapping builds" }),
  );
  expect(screen.getByText(/write reservations/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0]?.[0]).toMatchObject({
    trigger: {
      kind: "and",
      children: [{ kind: "cron", timezone: "Asia/Dubai" }, { kind: "cron" }],
    },
    policies: { allow_overlapping_builds: true },
  });
  workspace.dispose();
});
