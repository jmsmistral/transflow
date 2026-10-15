import {
  render,
  act,
  screen,
  fireEvent,
  waitFor,
  within,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { ApiFailure, Client } from "./api/client";
import { Workspace } from "./workspace";
import { Schedules } from "./Schedules";
import { ScheduleEditor } from "./ScheduleEditor";
import { ScheduleState } from "./ScheduleDetails";
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
  ApiLineageNodeV1,
  PlanResultV1,
} from "./generated/contracts";
import {
  related,
  savedDefinition,
  identity,
  duration,
  editGraphRoles,
  triggerDatasets,
  connectionIssue,
  usesTriggerConnections,
  withTriggerConnections,
  secondsLabel,
  type ScheduleEditSession,
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
const schedulePlan: PlanResultV1 = {
  kind: "plan",
  workspace: workspaceId,
  source: workspaceId,
  source_digest: "a".repeat(64),
  branch: "master",
  source_selector: { kind: "working_tree" },
  source_commit: null,
  plan_id: id,
  digest: "b".repeat(64),
  created_us: "1",
  expires_us: "9999999999999999",
  mode: "Full",
  force: false,
  targets: ["raw/example"],
  boundaries: [],
  exclusions: [],
  refresh_sources: [],
  fallback_override: null,
  boundary_policy: "require_available",
  branch_would_be_created: false,
  pending_registrations: [],
  writes: [
    {
      dataset: datasetId,
      path: "raw/example",
      job: id,
      expected_generation: "0",
      bindings: [],
      parameters: {},
      resources: {
        timeout_seconds: "3600",
        validation_timeout_seconds: "3600",
        discovery_timeout_seconds: "60",
        interactive_timeout_seconds: "60",
        max_jobs: "4",
        cpu_tokens: "1",
        worker_threads: "1",
        memory_budget_mib: null,
        memory_origin: null,
        memory_enforcement: "none",
        timeout_origin: "Default",
        validation_timeout_origin: "Default",
        discovery_timeout_origin: "Default",
        interactive_timeout_origin: "Default",
        max_jobs_origin: "Default",
        cpu_origin: "Default",
      },
    },
  ],
  reads: [],
  freshness: [],
  freshness_context: "selected_branch_heads",
  warnings: [],
  producer_execution: false,
  authoring_changed: false,
};
async function setup(
  conflict = false,
  withHistory = false,
  advanceContext = false,
) {
  let revision = 1;
  const mutations: { method: string; headers: Headers; body: unknown }[] = [];
  const base = fixtureTransport((path, query) => {
    if (path === "/api/v1/schedules")
      return Promise.resolve(
        json({ schedules: [record], next_cursor: null }, null),
      );
    if (path.endsWith("/roles")) return Promise.resolve(json(roles, context()));
    if (path.endsWith("/history"))
      return Promise.resolve(
        json(
          {
            schedule_id: id,
            occurrences: withHistory
              ? [
                  {
                    id,
                    ready_us: String(Date.now() * 1000),
                    disposition: "SUCCEEDED",
                    build: workspaceId,
                    branch: "historical-branch",
                    build_state: "SUCCEEDED",
                    manual: true,
                    started_us: "1700000000000000",
                    finished_us: "1700000001000000",
                    wall_duration_us: "1000000",
                    jobs: "1",
                    attempts: "1",
                    coalesced_tokens: "0",
                    plan: datasetId,
                    source: workspaceId,
                  },
                ]
              : [],
            next_cursor: null,
          },
          null,
        ),
      );
    if (path.endsWith("/metrics") && withHistory)
      return Promise.resolve(
        json(
          {
            schedule_id: id,
            from_us: query.from_us,
            to_us: query.to_us,
            cohort: "occurrence_ready",
            duration_unit: "us",
            occurrences: "1",
            dispositions: [{ state: "SUCCEEDED", count: "1" }],
            builds: "1",
            jobs: "1",
            attempts: "1",
            failure_rate: { numerator: "0", denominator: "1" },
            duration_samples: "1",
            missing_duration_samples: "0",
            median_us: { numerator: "1000000", denominator: "1" },
            trailing_mean_us: { numerator: "1000000", denominator: "1" },
            trailing_window: "10",
            trailing_samples: "1",
            ignored_event_matches: "0",
            clock_ticks: [],
            evidence_cohort: "observed_in_range",
          },
          null,
        ),
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
    if (String(url).startsWith("/api/v1/schedules/preview?"))
      return json(schedulePlan, context());
    if (String(url) === "/api/v1/schedules/clock-preview")
      return json({ leaves: [] }, null);
    if (String(url) === `/api/v1/schedules/${id}`) {
      if (advanceContext) revision++;
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
    const response = await base(url, init);
    if (advanceContext && String(url) === "/api/v1/read") {
      const request = JSON.parse(String(init?.body)) as {
        path: string;
        query: Record<string, string>;
      };
      const value = (await response.json()) as {
        context: ReturnType<typeof context> | null;
        data: unknown;
      };
      const live = {
        ...context(),
        runtime_revision: String(revision),
        fingerprint: (revision === 1 ? "d" : "e").repeat(64),
      };
      if (request.query.context && request.query.context !== live.fingerprint)
        return new Response(
          JSON.stringify({
            error: { message: "The selected context changed" },
          }),
          { status: 409 },
        );
      return json(
        request.path === "/api/v1/context" ? live : value.data,
        value.context ? live : null,
      );
    }
    return response;
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
  await screen.findByText("1 dataset");
  expect(screen.queryByText(/Saved roles:/)).toBeNull();
  expect(onPlan).toHaveBeenCalledWith(schedulePlan);
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
    />,
  );
  fireEvent.change(screen.getByLabelText("Schedule name"), {
    target: { value: "Edited" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(cancel).not.toHaveBeenCalled();
  expect(mutations.length).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  expect(cancel).toHaveBeenCalled();
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

test("focused schedule hides list controls, opens complete details and returns to the list", async () => {
  const { workspace, state } = await setup();
  const rendered = render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[]}
      visible={[identity(target)]}
      onRoles={vi.fn()}
      onPlan={vi.fn()}
      onAdd={vi.fn()}
    />,
  );
  expect(screen.getByRole("button", { name: "New schedule" })).toBeTruthy();
  expect(
    screen.getByRole("textbox", { name: "Search schedules" }),
  ).toBeTruthy();
  fireEvent.click(await screen.findByText("Daily review"));
  expect(screen.queryByRole("button", { name: "New schedule" })).toBeNull();
  expect(
    screen.queryByRole("textbox", { name: "Search schedules" }),
  ).toBeNull();
  expect(
    screen.queryByRole("switch", { name: "All workspace schedules" }),
  ).toBeNull();
  expect(screen.queryByText("Build scope")).toBeNull();
  expect(screen.queryByText("Missed ticks")).toBeNull();
  const viewButton = screen.getByRole("button", { name: "View schedule" });
  viewButton.focus();
  fireEvent.click(viewButton);
  const dialog = await screen.findByRole("dialog", {
    name: "Schedule details",
  });
  expect(within(dialog).getByText("Build scope")).toBeTruthy();
  expect(within(dialog).getByText("Missed ticks")).toBeTruthy();
  expect(within(dialog).getByText(workspaceId)).toBeTruthy();
  expect(within(dialog).getByText(/code used to build/)).toBeTruthy();
  expect(
    within(dialog).getByRole("heading", {
      name: "Recent builds and occurrences",
    }),
  ).toBeTruthy();
  fireEvent.click(
    within(dialog).getByRole("button", { name: "Close schedule view" }),
  );
  expect(screen.queryByRole("dialog", { name: "Schedule details" })).toBeNull();
  expect(document.activeElement).toBe(viewButton);
  fireEvent.click(screen.getByRole("button", { name: "Schedules" }));
  expect(
    screen.getByRole("textbox", { name: "Search schedules" }),
  ).toBeTruthy();
  rendered.unmount();
  workspace.dispose();
});

test("recent schedule build uses the retained plan and historical branch; closing restores its schedule view", async () => {
  const { workspace, state } = await setup(false, true);
  const scope = vi
    .spyOn(workspace, "scope")
    .mockRejectedValue(new Error("Retained build unavailable"));
  const rendered = render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[]}
      visible={[identity(target)]}
      onRoles={vi.fn()}
      onPlan={vi.fn()}
      onAdd={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByText("Daily review"));
  fireEvent.click(screen.getByRole("button", { name: "View schedule" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Schedule details",
  });
  fireEvent.click(
    await within(dialog).findByRole("button", { name: "View build" }),
  );
  const build = await screen.findByRole("dialog", { name: "Build report" });
  await waitFor(() =>
    expect(scope).toHaveBeenCalledWith(
      { branch: "historical-branch", plan: datasetId },
      expect.any(AbortSignal),
    ),
  );
  expect(within(build).getByRole("alert").textContent).toContain(
    "Retained build unavailable",
  );
  expect(workspace.snapshot()).toMatchObject({
    selection: { branch: "master" },
  });
  fireEvent(build, new Event("cancel", { bubbles: true, cancelable: true }));
  expect(screen.getByRole("dialog", { name: "Schedule details" })).toBe(dialog);
  rendered.unmount();
  workspace.dispose();
});

test("schedule editor keeps Save and Cancel before the fields", async () => {
  const { workspace, state } = await setup();
  const rendered = render(
    <ScheduleEditor
      workspace={workspace}
      initial={definition}
      record={record}
      datasets={state.value.datasets.entries}
      schedules={[record]}
      onSave={vi.fn()}
      onCancel={vi.fn()}
    />,
  );
  const header = rendered.container.querySelector(".schedule-editor-header");
  if (!header) throw Error("Missing editor header");
  expect(
    within(header as HTMLElement).getByRole("button", {
      name: "Save schedule",
    }),
  ).toBeTruthy();
  expect(
    within(header as HTMLElement).getByRole("button", { name: "Cancel" }),
  ).toBeTruthy();
  expect(
    header.compareDocumentPosition(screen.getByLabelText("Schedule name")) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  rendered.unmount();
  workspace.dispose();
});

const graphNode = (dataset_id: string, external = false): ApiLineageNodeV1 => ({
  identity: identity({ workspace_id: workspaceId, dataset_id }),
  paths: [`data/${dataset_id}`],
  depth: "0",
  external,
  producer: !external,
  resource_type: "source",
  publication: "unknown",
  parent_count: "0",
  child_count: "0",
});
test("graph role drafts retain qualified identities and untouched typed trigger leaves; duplicates do not create extra leaves", () => {
  const second = graphNode(id),
    foreign = {
      ...graphNode(datasetId, true),
      identity: identity({ workspace_id: id, dataset_id: datasetId }),
    };
  const added = editGraphRoles(
    definition,
    "trigger",
    [second, foreign, second],
    () => "added",
  );
  expect(added.trigger.kind).toBe("and");
  if (added.trigger.kind !== "and") throw Error("compound trigger");
  expect(added.trigger.children[0]).toBe(definition.trigger);
  expect(added.trigger.children[1]).toMatchObject({
    id: "added",
    branch: "output",
    payload_mode: "pin",
  });
  expect(triggerDatasets(added.trigger)).toHaveLength(3);
  expect(editGraphRoles(added, "trigger", [second])).toBe(added);
  const targets = editGraphRoles(definition, "target", [
    second,
    foreign,
    second,
  ]);
  expect(targets.build.targets).toHaveLength(2);
  expect(targets.build.source).toBe(definition.build.source);
  expect(definition.build.targets).toEqual([target]);
});
test("Clear removes dataset membership and normalizes nested trigger groups while retaining remaining IDs and clock policy", () => {
  const second = graphNode(id),
    clock = {
      kind: "cron",
      id: "clock",
      expression: "0 9 * * *",
      timezone: "Asia/Dubai",
      duplicate_time: "both",
    } as const;
  const compound: ScheduleDefinitionV1 = {
    ...definition,
    trigger: {
      kind: "or",
      children: [
        clock,
        {
          kind: "and",
          children: [
            definition.trigger,
            {
              kind: "dataset_published",
              id: "second",
              dataset: { workspace_id: workspaceId, dataset_id: id },
              branch: "input",
              payload_mode: "pin",
              include_resets: true,
            },
          ],
        },
      ],
    },
  };
  const cleared = editGraphRoles(compound, "clear", [
    graphNode(datasetId),
    second,
  ]);
  expect(cleared.trigger).toEqual(clock);
  expect(cleared.build.targets).toEqual([]);
  expect(
    editGraphRoles(definition, "clear", [graphNode(datasetId)]).trigger,
  ).toEqual({ kind: "manual" });
  expect(secondsLabel(86400)).toBe("24 h");
  expect(secondsLabel(3661)).toBe("1 h 1 min 1 s");
  expect(secondsLabel(60)).toBe("1 min");
  expect(secondsLabel(0)).toBe("0 s");
});
test("graph editing shows only targets, changes no saved state before Save, and Exit can keep editing or save with the original ETag", async () => {
  const { workspace, state, mutations } = await setup();
  let session: ScheduleEditSession | null = null;
  const cancel = vi.fn(),
    second = graphNode(id);
  const onEdit = (s: ScheduleEditSession | null) => {
    session = s;
  };
  const save = vi.fn(async (d: ScheduleDefinitionV1, signal: AbortSignal) => {
    await workspace.client.saveSchedule(d, record, signal);
  });
  render(
    <ScheduleEditor
      workspace={workspace}
      initial={definition}
      record={record}
      datasets={state.value.datasets.entries}
      schedules={[record]}
      selected={[second]}
      onEdit={onEdit}
      onSave={save}
      onCancel={cancel}
    />,
  );
  expect(
    screen.queryByRole("button", {
      name: "Use workspace source and input defaults",
    }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Add 1 selected node" }));
  expect(
    within(
      screen.getByRole("region", { name: "Datasets to build" }),
    ).getAllByRole("listitem"),
  ).toHaveLength(2);
  expect(mutations).toHaveLength(0);
  if (!session) throw Error("missing edit session");
  const edit = session as ScheduleEditSession;
  act(() => edit.act("trigger", [second]));
  expect(screen.getAllByLabelText("Condition ID")).toHaveLength(2);
  act(() => edit.exit());
  await screen.findByRole("dialog", { name: "Save schedule changes?" });
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect(cancel).not.toHaveBeenCalled();
  expect(mutations).toHaveLength(0);
  act(() => edit.exit());
  fireEvent.click(screen.getByRole("button", { name: "Save and exit" }));
  await waitFor(() => expect(mutations).toHaveLength(1));
  expect(save.mock.calls[0]?.[0].build.targets).toHaveLength(2);
  expect(mutations[0]?.headers.get("If-Match")).toBe(`"${record.etag}"`);
  workspace.dispose();
});

test("guarded schedule Save refreshes its advanced runtime fence before graph selection and live branch reads", async () => {
  const { workspace, state, mutations } = await setup(false, false, true);
  render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[]}
      visible={[identity(target)]}
      onRoles={vi.fn()}
      onPlan={vi.fn()}
      onAdd={vi.fn()}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Daily review Paused output" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  fireEvent.change(screen.getByLabelText("Description"), {
    target: { value: "Reviewed" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  await waitFor(() =>
    expect(screen.getByRole("status").textContent).toContain("Schedule saved"),
  );
  expect(mutations).toHaveLength(1);
  const refreshed = workspace.snapshot();
  if (refreshed.kind !== "ready")
    throw Error("save lost the verified workspace");
  expect(refreshed.value.context.runtime_revision).toBe("2");
  await expect(
    workspace.read("ApiMetadataPageV1", "/api/v1/branches"),
  ).resolves.toMatchObject({
    entries: [{ name: "master" }, { name: "feature" }],
  });
  workspace.dispose();
});

test("schedule statuses distinguish pause, active and live execution, including paused manual work", () => {
  const view = render(<ScheduleState record={record} />);
  expect(screen.getByText("Paused")).toBeTruthy();
  expect(
    view.container.querySelector(".schedule-state-paused .icon"),
  ).toBeTruthy();
  view.rerender(<ScheduleState record={{ ...record, paused: false }} />);
  expect(screen.getByText("Active")).toBeTruthy();
  view.rerender(
    <ScheduleState
      record={{ ...record, paused: false, currently_running: true }}
    />,
  );
  expect(screen.getByText("Active and currently running")).toBeTruthy();
  expect(
    view.container.querySelector(".schedule-state-running .icon"),
  ).toBeTruthy();
  view.rerender(
    <ScheduleState record={{ ...record, currently_running: true }} />,
  );
  expect(screen.getByText("Automatic scheduling paused")).toBeTruthy();
  view.rerender(<ScheduleState record={record} />);
  expect(screen.queryByText("Active and currently running")).toBeNull();
});

test("saved schedule scope includes connecting and off-canvas jobs in the sidebar, modal and graph plan without repeating planning on status polls", async () => {
  const { workspace, state } = await setup();
  const write = schedulePlan.writes[0];
  if (!write) throw Error("missing scope fixture");
  const full = {
    ...schedulePlan,
    writes: [
      ...schedulePlan.writes,
      { ...write, dataset: workspaceId, path: "curated/connecting" },
    ],
  };
  const preview = vi
    .spyOn(workspace.client, "schedulePreview")
    .mockResolvedValue(full);
  const onPlan = vi.fn();
  const props = {
    workspace,
    state,
    selected: [],
    visible: [identity(target)],
    onRoles: vi.fn(),
    onPlan,
    onAdd: vi.fn(),
  };
  const view = render(<Schedules {...props} />);
  fireEvent.click(await screen.findByText("Daily review"));
  await screen.findByText("2 datasets");
  expect(
    within(
      screen.getByRole("region", { name: "Datasets to build" }),
    ).getAllByRole("listitem"),
  ).toHaveLength(2);
  expect(screen.getByText("curated/connecting")).toBeTruthy();
  expect(onPlan).toHaveBeenCalledWith(full);
  expect(preview).toHaveBeenCalledWith(
    definition,
    state.value.context,
    expect.any(AbortSignal),
  );
  const headings = [...view.container.querySelectorAll("h4")].map(
    (n) => n.textContent,
  );
  expect(headings.indexOf("Code and build options")).toBeGreaterThan(
    headings.indexOf("Branches"),
  );
  fireEvent.click(screen.getByRole("button", { name: "View schedule" }));
  const modal = screen.getByRole("dialog", { name: "Schedule details" });
  expect(within(modal).getByText("2 datasets")).toBeTruthy();
  expect(within(modal).getByText("curated/connecting")).toBeTruthy();
  view.rerender(
    <Schedules
      {...props}
      state={{
        ...state,
        value: {
          ...state.value,
          context: { ...state.value.context, runtime_revision: "2" },
        },
      }}
    />,
  );
  expect(preview).toHaveBeenCalledTimes(1);
  view.unmount();
  workspace.dispose();
});

test("unavailable schedule scope never substitutes explicit targets or a zero count", async () => {
  const { workspace, state } = await setup();
  vi.spyOn(workspace.client, "schedulePreview").mockRejectedValue(
    Error("Retained source unavailable"),
  );
  const onPlan = vi.fn();
  const view = render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[]}
      visible={[identity(target)]}
      onRoles={vi.fn()}
      onPlan={onPlan}
      onAdd={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByText("Daily review"));
  await screen.findByText(
    /Schedule build scope unavailable.*Retained source unavailable/,
  );
  const scope = screen.getByRole("region", { name: "Datasets to build" });
  expect(within(scope).queryAllByRole("listitem")).toHaveLength(0);
  expect(within(scope).queryByText("0 datasets")).toBeNull();
  expect(onPlan.mock.calls.every(([plan]) => plan === null)).toBe(true);
  view.unmount();
  workspace.dispose();
});

test("leaving a schedule discards a late scope response without repainting the graph", async () => {
  const { workspace, state } = await setup();
  let finish: (plan: PlanResultV1) => void = () => {
    throw Error("not started");
  };
  vi.spyOn(workspace.client, "schedulePreview").mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const onPlan = vi.fn();
  const view = render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[]}
      visible={[identity(target)]}
      onRoles={vi.fn()}
      onPlan={onPlan}
      onAdd={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByText("Daily review"));
  fireEvent.click(screen.getByRole("button", { name: "Schedules" }));
  await act(async () => {
    finish(schedulePlan);
  });
  expect(onPlan.mock.calls.every(([plan]) => plan === null)).toBe(true);
  expect(screen.queryByText("1 dataset")).toBeNull();
  view.unmount();
  workspace.dispose();
});

test("a runtime read conflict refreshes context and automatically recovers scope without an Update preview button", async () => {
  const { workspace, state } = await setup();
  const preview = vi
    .spyOn(workspace.client, "schedulePreview")
    .mockRejectedValueOnce(new ApiFailure("conflict", "Context advanced"))
    .mockResolvedValue(schedulePlan);
  const originalRefresh = workspace.refresh;
  const refresh = vi
    .spyOn(workspace, "refresh")
    .mockRejectedValueOnce(
      new ApiFailure("conflict", "Read fence moved during refresh"),
    )
    .mockImplementation(originalRefresh);
  const view = render(
    <Schedules
      workspace={workspace}
      state={state}
      selected={[]}
      visible={[identity(target)]}
      onRoles={vi.fn()}
      onPlan={vi.fn()}
      onAdd={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByText("Daily review"));
  await screen.findByText("1 dataset", {}, { timeout: 3000 });
  expect(refresh).toHaveBeenCalledWith(true);
  expect(refresh).toHaveBeenCalledTimes(2);
  expect(preview).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("button", { name: "Update preview" })).toBeNull();
  view.unmount();
  workspace.dispose();
});

test("connecting builds bind published triggers as read boundaries and retain separate target identities", () => {
  const upstream = { workspace_id: workspaceId, dataset_id: id };
  const draft: ScheduleDefinitionV1 = {
    ...definition,
    trigger: {
      ...definition.trigger,
      kind: "dataset_published",
      id: "input",
      dataset: upstream,
      branch: "input",
      payload_mode: "pin",
      include_resets: false,
    },
  };
  const connected = withTriggerConnections(draft, true);
  expect(connectionIssue(connected)).toBe("");
  expect(connected.build).toMatchObject({
    build_mode: "between",
    targets: [target],
    boundaries: [upstream],
  });
  expect(usesTriggerConnections(connected)).toBe(true);
  expect(connected.trigger).toBe(draft.trigger);
  expect(connected.build.source).toBe(draft.build.source);
  const explicit = withTriggerConnections(connected, false);
  expect(explicit.build).toMatchObject({
    build_mode: "selected",
    targets: [target],
    boundaries: [],
  });
  expect(usesTriggerConnections(explicit)).toBe(false);
  expect(connectionIssue({ ...draft, trigger: { kind: "manual" } })).toContain(
    "dataset trigger",
  );
  expect(
    connectionIssue({ ...draft, build: { ...draft.build, targets: [] } }),
  ).toContain("target dataset");
  expect(connectionIssue(definition)).toContain("targets different");
});

test("Connecting build is unavailable for manual/time-only schedules but those schedules can save selected targets", async () => {
  const { workspace } = await setup();
  const save = vi.fn<(definition: ScheduleDefinitionV1) => Promise<void>>(
    async () => undefined,
  );
  const initial = { ...definition, trigger: { kind: "manual" as const } };
  render(
    <ScheduleEditor
      workspace={workspace}
      initial={initial}
      record={null}
      datasets={[]}
      schedules={[]}
      onSave={save}
      onCancel={vi.fn()}
    />,
  );
  expect(
    screen
      .getByRole("switch", { name: "Connecting build" })
      .hasAttribute("disabled"),
  ).toBe(true);
  expect(
    screen.getByText(
      /Manual and time conditions can still build your selected targets/,
    ),
  ).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(save.mock.calls[0]?.[0]).toMatchObject({
    trigger: { kind: "manual" },
    build: { build_mode: "selected", targets: [target] },
  });
  workspace.dispose();
});

test("connecting toggle follows graph trigger edits, blocks incomplete saves and can return to explicit targets", async () => {
  const { workspace } = await setup();
  const upstream = { workspace_id: workspaceId, dataset_id: id };
  const initial: ScheduleDefinitionV1 = {
    ...definition,
    trigger: {
      kind: "dataset_published",
      id: "input",
      dataset: upstream,
      branch: "input",
      payload_mode: "pin",
      include_resets: false,
    },
  };
  const save = vi.fn<(definition: ScheduleDefinitionV1) => Promise<void>>(
    async () => undefined,
  );
  let session: ScheduleEditSession | null = null;
  render(
    <ScheduleEditor
      workspace={workspace}
      initial={initial}
      record={null}
      datasets={[]}
      schedules={[]}
      onSave={save}
      onCancel={vi.fn()}
      onEdit={(s) => {
        session = s;
      }}
    />,
  );
  fireEvent.click(screen.getByRole("switch", { name: "Connecting build" }));
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0]?.[0].build).toMatchObject({
    build_mode: "between",
    boundaries: [upstream],
    targets: [target],
  });
  const current = session as ScheduleEditSession | null;
  if (!current) throw Error("missing editing session");
  act(() => current.act("clear", [graphNode(id)]));
  expect(
    screen
      .getByRole("switch", { name: "Connecting build" })
      .getAttribute("aria-checked"),
  ).toBe("true");
  expect(
    screen
      .getByRole("button", { name: "Save schedule" })
      .hasAttribute("disabled"),
  ).toBe(true);
  expect(screen.getByRole("alert").textContent).toContain("dataset trigger");
  fireEvent.click(screen.getByRole("switch", { name: "Connecting build" }));
  fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1]?.[0]).toMatchObject({
    trigger: { kind: "manual" },
    build: { build_mode: "selected", targets: [target], boundaries: [] },
  });
  workspace.dispose();
});
