import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test } from "vitest";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import { CodeInspector } from "./CodeInspector";
import { HistoryInspector } from "./HistoryInspector";
import { TimelineInspector } from "./BuildInspectors";
import { PreviewInspector } from "./inspectors";
import {
  context,
  dataset,
  datasetId,
  fixtureTransport,
  json,
  versionId,
  workspaceId,
} from "./test-fixtures";
import type {
  ApiContextV1,
  ApiExecutionMetricsV1,
  ApiHistoryJobV1,
  ApiVersionsV1,
} from "./generated/contracts";
const plan = "11111111-1111-4111-8111-111111111111",
  build = "22222222-2222-4222-8222-222222222222",
  attempt = "33333333-3333-4333-8333-333333333333",
  jobId = "44444444-4444-4444-8444-444444444444";
const phase = {
  phase: "Running",
  started_us: "1700000000000000",
  finished_us: "1700000001000000",
  duration_ns: "1000000000",
};
const job: ApiHistoryJobV1 = {
  id: jobId,
  build,
  build_state: "FAILED",
  build_job_count: "1",
  plan,
  source: workspaceId,
  state: "FAILED",
  created_us: "1700000000000000",
  finished_us: "1700000001000000",
  attempt_count: "1",
  produced_version: null,
  reused_version: null,
  original_attempt: null,
  duration_ns: "1000000000",
  attempts: [
    {
      attempt,
      number: "1",
      state: "FAILED",
      started_us: phase.started_us,
      finished_us: phase.finished_us,
      duration_ns: phase.duration_ns,
      phases: [phase],
    },
  ],
};
const versions: ApiVersionsV1 = {
  entries: [
    {
      version: versionId,
      source: workspaceId,
      published_at_us: "1700000000000000",
      artifact: "a".repeat(64),
      schema: null,
      attempt: null,
      import: null,
      origin_workspace: workspaceId,
      dataset: datasetId,
      availability: "not_verified",
      origin: "local",
      row_count: "2",
      byte_count: "42",
      integrity_state: "VERIFIED",
    },
  ],
  next_cursor: null,
};
const metrics: ApiExecutionMetricsV1 = {
  from_us: "0",
  to_us: "1800000000000000",
  time_zone: "UTC",
  duration_unit: "ns",
  cohort: "build_accepted",
  branch: "master",
  dataset: datasetId,
  materialized_any: false,
  builds: "1",
  build_states: [{ state: "FAILED", count: "1" }],
  manual_requests: "1",
  scheduled_builds: "0",
  schedule_occurrences: null,
  jobs: "1",
  job_states: [{ state: "FAILED", count: "1" }],
  jobs_executed: "1",
  attempts: "1",
  failure_rate: { numerator: "1", denominator: "1" },
  materializations: "0",
  duration_samples: "0",
  missing_duration_samples: "0",
  median_ns: null,
  trailing_mean_ns: null,
  trailing_window: "10",
  trailing_samples: "0",
};
async function fixture(missing = false, historyJob: ApiHistoryJobV1 = job) {
  const calls: { path: string; query: Record<string, string> }[] = [];
  const model = new Workspace(
    new Client(
      fixtureTransport((path, query) => {
        calls.push({ path, query });
        let c: ApiContextV1 = context(query.branch);
        if (query.plan)
          c = {
            ...c,
            selection: { kind: "plan", id: query.plan, digest: "a".repeat(64) },
            fingerprint: "a".repeat(64),
          };
        if (query.version)
          c = {
            ...c,
            selection: {
              kind: "local_version",
              id: query.version,
              digest: "f".repeat(64),
            },
            fingerprint: "f".repeat(64),
          };
        if (path.endsWith("/context")) return Promise.resolve(json(c, c));
        if (path.endsWith(`/datasets/${datasetId}`))
          return Promise.resolve(
            json(
              {
                ...dataset,
                producer: missing
                  ? null
                  : {
                      path: query.version
                        ? "src/producing.py"
                        : "src/current.py",
                    },
              },
              c,
            ),
          );
        if (path.endsWith("/versions"))
          return Promise.resolve(json(versions, c));
        if (path.includes("/source/"))
          return Promise.resolve(
            json(
              {
                source: workspaceId,
                path: query.path,
                offset: 0,
                text: query.version
                  ? "# producing\ndef items(): return 1"
                  : "# current\ndef items(): return 2",
                next_offset: null,
                git: { branch: "master", commit: "abcd", dirty: true },
              },
              c,
            ),
          );
        if (path.endsWith("/history"))
          return Promise.resolve(
            json({ entries: [historyJob], next_cursor: null }, c),
          );
        if (path.endsWith("/metrics"))
          return Promise.resolve(
            json({ ...metrics, from_us: query.from_us, to_us: query.to_us }, c),
          );
        if (path.endsWith("/timeline"))
          return Promise.resolve(
            json(
              {
                build,
                state: "FAILED",
                plan,
                source: workspaceId,
                queued_us: phase.started_us,
                started_us: phase.started_us,
                finished_us: phase.finished_us,
                wall_duration_us: "1000000",
                initial_queue_wait_us: "0",
                resource_wait_us: null,
                eta_us: null,
                critical_path: { duration_ns: "1000000000", jobs: [jobId] },
                job_counts: [{ state: "FAILED", count: "1" }],
                jobs: [
                  {
                    job: jobId,
                    dataset: datasetId,
                    state: "FAILED",
                    duration_ns: "1000000000",
                    attempts: job.attempts,
                  },
                ],
              },
              c,
            ),
          );
        if (path.endsWith(`/builds/${build}`))
          return Promise.resolve(
            json(
              {
                id: build,
                requested_by: "fixture",
                trigger: { kind: "manual" },
                plan: {
                  writes: [{ dataset: datasetId, path: dataset.path }],
                  targets: [dataset.path],
                },
              },
              c,
            ),
          );
        if (path.endsWith(`/attempts/${attempt}`))
          return Promise.resolve(
            json(
              {
                attempt,
                build,
                job: jobId,
                dataset: datasetId,
                plan,
                source: workspaceId,
                source_availability: "retained",
                parameters: { cutoff: 42 },
                inputs: [],
                evidence: {
                  state: "FAILED",
                  failure_class: "USER_CODE",
                  phases: [phase],
                  checks: [],
                  report: { exit_code: 1 },
                },
                duration_ns: "1000000000",
                log_command: "transflow build logs",
              },
              c,
            ),
          );
        if (path.endsWith("/logs"))
          return Promise.resolve(
            json(
              {
                attempt,
                streams: [{ helper: "worker", stream: "stderr", bytes: "5" }],
                helper: "worker",
                stream: "stderr",
                offset: "0",
                text: "error",
                next_offset: null,
                available: true,
              },
              c,
            ),
          );
        return undefined;
      }),
    ),
  );
  await model.connect("launch");
  await model.select({
    branch: "master",
    dataset: datasetId,
    origin: workspaceId,
  });
  const state = model.snapshot();
  if (state.kind !== "ready") throw new Error("fixture not ready");
  return { model, state, calls };
}
test("producing source uses the exact version definition, current and diff are explicit", async () => {
  const { model, state, calls } = await fixture();
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText("# producing")).toBeTruthy();
  expect(screen.queryByText("# current")).toBeNull();
  expect(screen.getByText("Dirty capture")).toBeTruthy();
  expect(calls.find((c) => c.path.includes("/source/"))?.query).toMatchObject({
    version: versionId,
    path: "src/producing.py",
  });
  fireEvent.click(
    screen.getByRole("button", { name: "Current retained definition" }),
  );
  expect(await screen.findByText("# current")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Diff" }));
  expect(await screen.findByText("− # producing")).toBeTruthy();
  expect(screen.getByText("+ # current")).toBeTruthy();
  model.dispose();
});
test("missing retained producer is unavailable and never replaced with current code", async () => {
  const { model, state, calls } = await fixture(true);
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText(/Source unavailable/)).toBeTruthy();
  expect(calls.some((c) => c.path.includes("/source/"))).toBe(false);
  expect(screen.getByText(/Producing snapshot/)).toBeTruthy();
  model.dispose();
});
test("an explicitly pinned version is labelled as a version, not a missing branch", async () => {
  const { model, state } = await fixture();
  const pinned = {
    ...state,
    selection: { ...state.selection, version: versionId },
  };
  render(
    <>
      <PreviewInspector workspace={model} state={pinned} dark={false} />
      <CodeInspector workspace={model} state={pinned} />
      <HistoryInspector workspace={model} state={pinned} onVersion={() => {}} />
    </>,
  );
  expect(await screen.findByText("# producing")).toBeTruthy();
  expect(
    screen.queryByText(/Could not find data on current branch/),
  ).toBeNull();
  expect(screen.getByText(/Branch public/)).toBeTruthy();
  model.dispose();
});
test("history shows job details and returns to the summary without changing version context", async () => {
  const { model, state } = await fixture();
  const pinned: string[] = [];
  render(
    <HistoryInspector
      workspace={model}
      state={state}
      onVersion={(v) => {
        if (v) pinned.push(v.version);
      }}
    />,
  );
  expect(await screen.findByText("100.00%")).toBeTruthy();
  const failure = screen.getByRole("button", { name: /FAILED · 1.000 s/ });
  fireEvent.click(failure);
  expect(await screen.findByText("Failure: USER_CODE")).toBeTruthy();
  expect(
    await screen.findByRole("textbox", { name: "Retained log excerpt" }),
  ).toHaveProperty("textContent", "error");
  expect(pinned).toEqual([]);
  expect(screen.queryByRole("button", { name: "Versions" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Runs" })).toBeNull();
  expect(screen.queryByText("Back to duration summary")).toBeNull();
  const summary = screen.getByRole("button", { name: "Show summary" });
  expect(summary).toHaveProperty("disabled", false);
  fireEvent.click(summary);
  expect(await screen.findByText("100.00%")).toBeTruthy();
  expect(screen.queryByText("Failure: USER_CODE")).toBeNull();
  expect(summary).toHaveProperty("disabled", true);
  expect(failure.getAttribute("aria-pressed")).toBe("false");
  expect(pinned).toEqual([]);
  model.dispose();
});
test("history identifies the selected branch and submits inclusive ISO calendar dates", async () => {
  const { model, state, calls } = await fixture();
  render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  expect(await screen.findByText("100.00%")).toBeTruthy();
  expect(screen.getByTitle("History branch").textContent).toContain("master");
  const from = screen.getByRole("textbox", { name: "From" });
  const to = screen.getByRole("textbox", { name: "To" });
  expect(from).toHaveProperty("placeholder", "YYYY-MM-DD");
  fireEvent.change(from, { target: { value: "2024-02-29" } });
  fireEvent.change(to, { target: { value: "2024-02-29" } });
  const apply = screen.getByRole("button", { name: "Apply" });
  expect(apply).toHaveProperty("disabled", false);
  fireEvent.click(apply);
  await waitFor(() =>
    expect(
      calls.filter((c) => c.path.endsWith("/metrics")).at(-1)?.query,
    ).toMatchObject({
      branch: "master",
      from_us: "1709164800000000",
      to_us: "1709251200000000",
    }),
  );
  expect(await screen.findByText(/2024-02-29 to 2024-02-29/)).toBeTruthy();
  expect(screen.queryByText(/Build acceptance cohort/)).toBeNull();
  fireEvent.change(to, { target: { value: "2024-02-28" } });
  expect(apply).toHaveProperty("disabled", true);
  model.dispose();
});
test("Gantt uses recorded phases and exposes retained failure evidence in the owning plan", async () => {
  const { model, state, calls } = await fixture();
  render(<TimelineInspector workspace={model} state={state} />);
  const bar = await screen.findByRole("button", {
    name: /attempt 1: Running, 1.000 s/,
  });
  expect(bar.getAttribute("style")).toContain("width: 100%");
  fireEvent.click(screen.getByRole("button", { name: /Critical path/ }));
  await waitFor(() =>
    expect(bar.closest(".gantt-row")?.classList.contains("critical-job")).toBe(
      true,
    ),
  );
  expect(screen.queryByRole("button", { name: "Cancel build" })).toBeNull();
  expect(
    calls
      .filter(
        (c) =>
          c.path.endsWith("/timeline") ||
          c.path.endsWith(`/attempts/${attempt}`),
      )
      .every((c) => c.query.plan === plan),
  ).toBe(true);
  model.dispose();
});

test.each([
  ["SUCCEEDED", "FAILED", "2", "2 jobs"],
  ["SUCCEEDED", "SUCCEEDED", "1", "1 job"],
  ["RUNNING", "RUNNING", "3", "3 jobs"],
])(
  "history separates job %s from build %s",
  async (jobState, buildState, count, label) => {
    const { model, state } = await fixture(false, {
      ...job,
      state: jobState,
      build_state: buildState,
      build_job_count: count,
    });
    render(
      <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
    );
    const line = await screen.findByTitle(`Build ${build}`);
    expect(line.textContent).toContain(`${buildState} build · ${label}`);
    expect(
      line.querySelector(`.status-${buildState.toLowerCase()}`),
    ).toBeTruthy();
    const entry = line.closest("button");
    expect(
      entry?.querySelector(`:scope > .status-${jobState.toLowerCase()}`),
    ).toBeTruthy();
    model.dispose();
  },
);
