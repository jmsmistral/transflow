import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { BuildPlanner } from "./BuildPlanner";
import { selection, resources, type Options } from "./planning";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import {
  context,
  datasetId,
  workspaceId,
  fixtureTransport,
  json,
} from "./test-fixtures";
import type {
  PlanResultV1,
  ApiContextV1,
  ApiLineageNodeV1,
} from "./generated/contracts";

const options: Options = {
  targets: "raw/example",
  mode: "full",
  boundaries: "",
  exclusions: "",
  refresh: "",
  pins: "",
  parameters: "",
  force: false,
  requireCurrent: false,
  gitRef: "",
};
const node: ApiLineageNodeV1 = {
  identity: `dataset:${workspaceId}:${datasetId}`,
  paths: ["raw/example"],
  depth: "0",
  external: false,
  producer: true,
  parent_count: "0",
  child_count: "0",
  publication: "published",
  resource_type: "polars_transform",
};
const planId = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const buildId = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const plan: PlanResultV1 = {
  kind: "plan",
  workspace: workspaceId,
  source: workspaceId,
  source_digest: "a".repeat(64),
  branch: "master",
  source_selector: { kind: "working_tree" },
  source_commit: null,
  plan_id: planId,
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
      job: buildId,
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
const planContext: ApiContextV1 = {
  ...context(),
  selection: { kind: "plan", id: planId, digest: plan.digest },
  fingerprint: "f".repeat(64),
};
async function setup({
  interrupt = false,
  conflict = false,
  expired = false,
  previewConflicts = 0,
  previewFailure = false,
} = {}) {
  const mutations: { path: string; body: unknown; headers: Headers }[] = [];
  const base = fixtureTransport((path, query) =>
    path === "/api/v1/context" && query.plan
      ? Promise.resolve(json(planContext, planContext))
      : undefined,
  );
  const transport: typeof fetch = async (url, init) => {
    const path = String(url);
    if (path.startsWith("/api/v1/plans?")) {
      mutations.push({
        path,
        body: JSON.parse(String(init?.body)),
        headers: new Headers(init?.headers),
      });
      if (previewConflicts-- > 0) return new Response("{}", { status: 409 });
      if (previewFailure) return new Response("{}", { status: 422 });
      const result = expired ? { ...plan, expires_us: "1" } : plan;
      expired = false;
      return json(result, context());
    }
    if (path.startsWith("/api/v1/builds?")) {
      mutations.push({
        path,
        body: JSON.parse(String(init?.body)),
        headers: new Headers(init?.headers),
      });
      if (interrupt) {
        interrupt = false;
        throw new Error("connection lost");
      }
      if (conflict) {
        conflict = false;
        return new Response("{}", { status: 409 });
      }
      return json(
        { build: buildId, plan: planId, state: "QUEUED" },
        planContext,
      );
    }
    return base(url, init);
  };
  const workspace = new Workspace(new Client(transport));
  await workspace.connect("test");
  const state = workspace.snapshot();
  if (state.kind !== "ready") throw new Error("fixture is not ready");
  const props = {
    workspace,
    state,
    selected: [node],
    visible: [],
    onAdd: vi.fn(),
    onSelect: vi.fn(),
    onPlan: vi.fn(),
  };
  const view = render(<BuildPlanner {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "Next (View preview)" }));
  if (previewFailure) await screen.findByRole("alert");
  else await screen.findByRole("region", { name: "Plan preview" });
  return { mutations, props, view, workspace };
}

describe("guarded build planner", () => {
  it("defaults to full ancestors and preserves explicit empty fallback and advanced selectors", () => {
    expect(selection(options, "master", null)).toMatchObject({
      mode: "full",
      force: false,
      fallbacks: null,
    });
    expect(
      selection(
        {
          ...options,
          mode: "between",
          boundaries: "raw/input\nraw/input",
          exclusions: "raw/skip",
          pins: "curated/out#input=version",
          parameters: '{"limit":10}',
          force: true,
        },
        "review",
        [],
      ),
    ).toMatchObject({
      branch: "review",
      mode: "between",
      boundaries: ["raw/input"],
      exclusions: ["raw/skip"],
      pins: ["curated/out#input=version"],
      parameters: { limit: 10 },
      force: true,
      fallbacks: [],
    });
    expect(() =>
      selection({ ...options, parameters: "[]" }, "master", null),
    ).toThrow("JSON object");
    expect(() =>
      selection({ ...options, targets: "" }, "master", null),
    ).toThrow("target");
  });
  it("accepts only the reviewed plan with its original guard and reuses a receipt after interruption", async () => {
    const { mutations, workspace } = await setup({ interrupt: true });
    fireEvent.click(screen.getByRole("button", { name: "Run build" }));
    const retry = await screen.findByRole("button", {
      name: "Retry build acceptance",
    });
    expect(
      screen.getByRole("switch", { name: /Force build/ }).matches(":disabled"),
    ).toBe(true);
    fireEvent.click(retry);
    await screen.findByRole("dialog", { name: "Build report" });
    const accepts = mutations.filter((m) =>
      m.path.startsWith("/api/v1/builds?"),
    );
    expect(accepts).toHaveLength(2);
    expect(accepts[0]?.body).toEqual({ kind: "plan", plan_id: planId });
    expect(accepts[1]?.headers.get("Idempotency-Key")).toEqual(
      accepts[0]?.headers.get("Idempotency-Key"),
    );
    expect(accepts[0]?.headers.get("If-Match")).toBe(
      `"${planContext.fingerprint}"`,
    );
    workspace.dispose();
  });
  it("automatically replans changed selection and guarded rejection without starting another build", async () => {
    const { props, view, mutations, workspace } = await setup({
      conflict: true,
    });
    const build = screen.getByRole("button", { name: "Run build" });
    view.rerender(
      <BuildPlanner
        {...props}
        selected={[{ ...node, paths: ["raw/other"] }]}
      />,
    );
    expect(build.matches(":disabled")).toBe(true);
    await waitFor(() => expect(build.matches(":disabled")).toBe(false));
    expect(mutations.at(-1)?.body).toMatchObject({ targets: ["raw/other"] });
    fireEvent.click(build);
    await waitFor(() =>
      expect(
        mutations.filter((m) => m.path.startsWith("/api/v1/plans?")),
      ).toHaveLength(3),
    );
    await waitFor(() => expect(build.matches(":disabled")).toBe(false));
    expect(
      mutations.filter((m) => m.path.startsWith("/api/v1/builds?")),
    ).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Update preview" })).toBeNull();
    workspace.dispose();
  });
  it("refreshes context and retries transient preview conflicts", async () => {
    const { mutations, workspace } = await setup({ previewConflicts: 1 });
    expect(mutations).toHaveLength(2);
    expect(screen.queryByRole("alert")).toBeNull();
    workspace.dispose();
  });
  it("keeps preparation errors visible without repeatedly retrying deterministic failures", async () => {
    const { mutations, workspace } = await setup({ previewFailure: true });
    expect(mutations).toHaveLength(1);
    expect(
      screen.getByRole("button", { name: "Run build" }).matches(":disabled"),
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Retry preview" }));
    await waitFor(() => expect(mutations).toHaveLength(2));
    workspace.dispose();
  });
  it("follows graph selection, clears the panel when empty, and replans force", async () => {
    const { props, view, mutations, workspace } = await setup();
    expect(mutations[0]?.body).toMatchObject({
      targets: ["raw/example"],
      mode: "selected",
      force: false,
    });
    fireEvent.click(screen.getByRole("switch", { name: /Force build/ }));
    await waitFor(() => expect(mutations).toHaveLength(2));
    expect(mutations[1]?.body).toMatchObject({
      targets: ["raw/example"],
      force: true,
    });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText("Advanced options")).toBeNull();
    view.rerender(<BuildPlanner {...props} selected={[]} />);
    expect(
      screen.getByText("Select valid resources to begin a build"),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Run build" })).toBeNull();
    view.rerender(<BuildPlanner {...props} />);
    fireEvent.click(screen.getByRole("radio", { name: /in between/ }));
    fireEvent.click(
      screen.getByRole("button", { name: "Next (View preview)" }),
    );
    await waitFor(() => expect(mutations).toHaveLength(3));
    expect(mutations[2]?.body).toMatchObject({
      mode: "connecting",
      boundaries: [],
    });
    workspace.dispose();
  });
  it("replaces an expired draft automatically without accepting it", async () => {
    const { workspace, mutations } = await setup({ expired: true });
    await waitFor(() => expect(mutations).toHaveLength(2));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Run build" }).matches(":disabled"),
      ).toBe(false),
    );
    expect(mutations.every((m) => m.path.startsWith("/api/v1/plans?"))).toBe(
      true,
    );
    workspace.dispose();
  });
  it("counts distinct hidden resources across reads and writes, preserving provider identities", () => {
    const read = {
      consumer: datasetId,
      consumer_path: "curated/out",
      alias: "input",
      dataset: datasetId,
      path: "external/input",
      origin_workspace: buildId,
      version: planId,
      artifact: "c".repeat(64),
      starting_branch: "master",
      resolved_branch: "public",
      resolution: "fallback",
      currentness: "unknown" as const,
    };
    expect(
      resources({
        ...plan,
        writes: [],
        reads: [read, { ...read, alias: "other" }],
      }),
    ).toEqual([
      {
        identity: `dataset:${buildId}:${datasetId}`,
        path: "external/input",
        pending: false,
      },
    ]);
  });
});
