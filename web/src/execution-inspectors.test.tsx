import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { App } from "./App";
import { readFileSync } from "node:fs";
import { URL as NodeURL } from "node:url";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import { CodeInspector } from "./CodeInspector";
import { HistoryInspector } from "./HistoryInspector";
import { useBuildEvidence, orderedJobs } from "./build-evidence";
import { BuildGantt, BuildModal } from "./BuildReport";
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
  ApiExecutionTimelineV1,
  ExecutionJsonV1,
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
async function fixture(
  missing = false,
  historyJob: ApiHistoryJobV1 = job,
  control: {
    timeline?: () => Partial<ApiExecutionTimelineV1>;
    metrics?: Partial<ApiExecutionMetricsV1>;
    trigger?: ExecutionJsonV1;
    fail?: boolean;
    sql?: boolean;
    factory?: boolean;
    pythonPreview?: boolean;
    boundedSource?: boolean;
    factorySql?: boolean;
    generatedSql?: boolean;
    jobs?: ApiHistoryJobV1[];
    emptyBranch?: string;
  } = {},
) {
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
                      ...(control.factory || control.factorySql
                        ? {
                            ...(control.factorySql ? { engine: "duckdb" } : {}),
                            registration_path: "src/registrations.py",
                            authoring: {
                              kind: control.factorySql ? "sql" : "frames",
                              exported: true,
                              definition_path: "src/factory.py",
                              ...(control.pythonPreview
                                ? {
                                    python_preview: query.version
                                      ? "# Published preview\ndef item(): return 'published'"
                                      : "# Saved preview\ndef item(): return 'current'",
                                  }
                                : {}),
                            },
                          }
                        : {}),
                      ...(control.sql
                        ? { sql_source: { path: "src/producing.sql" } }
                        : {}),
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
                offset: control.boundedSource ? Number(query.offset ?? "0") : 0,
                text: control.sql
                  ? "SELECT * FROM rows"
                  : query.version
                    ? "# producing\ndef items(): return 1"
                    : "# current\ndef items(): return 2",
                next_offset: control.boundedSource
                  ? Number(query.offset ?? "0") + 200
                  : null,
                git: { branch: "master", commit: "abcd", dirty: true },
              },
              c,
            ),
          );
        if (path.endsWith("/history"))
          return Promise.resolve(
            json(
              {
                entries:
                  query.branch === control.emptyBranch
                    ? []
                    : (control.jobs ?? [historyJob]),
                next_cursor: null,
              },
              c,
            ),
          );
        if (path.endsWith("/metrics"))
          return Promise.resolve(
            json(
              {
                ...metrics,
                ...control.metrics,
                from_us: query.from_us,
                to_us: query.to_us,
              },
              c,
            ),
          );
        if (path.endsWith("/timeline") && control.fail)
          return Promise.reject(new Error("offline"));
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
                ...control.timeline?.(),
              },
              c,
            ),
          );
        if (path.endsWith(`/builds/${build}`))
          return Promise.resolve(
            json(
              {
                id: build,
                jobs: [
                  {
                    id: jobId,
                    dataset: datasetId,
                    state: "FAILED",
                    attempts: [{ id: attempt, failure_class: "USER_CODE" }],
                  },
                ],
                requested_by: "fixture",
                trigger: control.trigger ?? { kind: "manual" },
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
                ...(control.generatedSql
                  ? {
                      generated_sql: {
                        ...JSON.parse(
                          readFileSync(
                            new NodeURL(
                              "../../schemas/fixtures/conformance.json",
                              import.meta.url,
                            ),
                            "utf8",
                          ),
                        ).find(
                          (value: { name: string }) =>
                            value.name === "generated-sql-evidence",
                        ).value,
                        attempt,
                        source: workspaceId,
                        sql: "SELECT region, COUNT(*) FROM deliveries GROUP BY region",
                      },
                    }
                  : {}),
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
test("latest saved source is first and default, exact published source and side-by-side diff stay explicit", async () => {
  const { model, state, calls } = await fixture();
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText("# current")).toBeTruthy();
  expect(screen.queryByText("Dynamically generated code")).toBeNull();
  expect(screen.queryByText("# producing")).toBeNull();
  expect(
    screen.getByText(/Could not find data on current branch/),
  ).toBeTruthy();
  expect(screen.getByText(/shared across data branches/)).toBeTruthy();
  expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual([
    "Latest saved version",
    "Data publish version",
    "Diff",
  ]);
  expect(
    calls.find((c) => c.path.includes("/source/"))?.query.version,
  ).toBeUndefined();
  fireEvent.click(screen.getByRole("button", { name: "Data publish version" }));
  expect(await screen.findByText("# producing")).toBeTruthy();
  expect(screen.getByText("src/producing.py")).toBeTruthy();
  expect(
    screen.getByText(/Could not find data on current branch/),
  ).toBeTruthy();
  expect(screen.getByText("def").className).toBe("syntax-keyword");
  expect(
    calls.filter((c) => c.path.includes("/source/")).at(-1)?.query,
  ).toMatchObject({ version: versionId, path: "src/producing.py" });
  fireEvent.click(screen.getByRole("button", { name: "Diff" }));
  await waitFor(() =>
    expect(
      document.querySelector(".source-diff-columns .diff-removed code")
        ?.textContent,
    ).toBe("− # producing"),
  );
  expect(
    document.querySelector(".source-diff-columns .diff-added code")
      ?.textContent,
  ).toBe("+ # current");
  expect(
    screen.getByRole("region", { name: "Data publish version" }),
  ).toBeTruthy();
  expect(
    screen.getByRole("region", { name: "Latest saved version" }),
  ).toBeTruthy();
  model.dispose();
});
test("SQL code uses the captured SQL file rather than its Python registration", async () => {
  const { model, state, calls } = await fixture(false, job, { sql: true });
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText("src/producing.sql")).toBeTruthy();
  expect(screen.getByText("SELECT").className).toBe("syntax-keyword");
  expect(calls.find((c) => c.path.includes("/source/"))?.query).toMatchObject({
    branch: "master",
    path: "src/producing.sql",
  });
  fireEvent.click(screen.getByRole("button", { name: "Data publish version" }));
  await waitFor(() =>
    expect(calls.filter((c) => c.path.includes("/source/")).length).toBe(2),
  );
  expect(
    calls
      .filter((c) => c.path.includes("/source/"))
      .every((c) => c.query.path === "src/producing.sql"),
  ).toBe(true);
  model.dispose();
});
test("missing retained producer is unavailable and never replaced with current code", async () => {
  const { model, state, calls } = await fixture(true);
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText(/Source unavailable/)).toBeTruthy();
  expect(calls.some((c) => c.path.includes("/source/"))).toBe(false);
  expect(screen.queryByText(/Producing snapshot/)).toBeNull();
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
  expect(await screen.findByText("# current")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Data publish version" }));
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
  const failure = screen.getByRole("button", { name: /FAILED · 1 attempts/ });
  fireEvent.click(failure);
  expect(await screen.findByText("Failure: USER_CODE")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Logs" }));
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
  expect(from).toHaveProperty("value", "2024-02-29");
  expect(to).toHaveProperty("value", "2024-02-29");
  expect(screen.queryByText(/Build acceptance cohort/)).toBeNull();
  fireEvent.change(to, { target: { value: "2024-02-28" } });
  expect(apply).toHaveProperty("disabled", true);
  model.dispose();
});
test("empty branch history reads fallback jobs, metrics and plan details under the labelled branch", async () => {
  const { model, calls } = await fixture(false, job, { emptyBranch: "review" });
  await model.select({
    branch: "review",
    dataset: datasetId,
    origin: workspaceId,
  });
  const state = model.snapshot();
  if (state.kind !== "ready") throw new Error("fixture not ready");
  render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  expect(await screen.findByText("100.00%")).toBeTruthy();
  expect(screen.getByTitle("History branch").textContent).toContain("public");
  expect(
    screen.getByText(/Could not find data on current branch/),
  ).toBeTruthy();
  expect(
    calls.filter((c) => c.path.endsWith("/history")).map((c) => c.query.branch),
  ).toEqual(["review", "public"]);
  expect(
    calls.filter((c) => c.path.endsWith("/metrics")).at(-1)?.query.branch,
  ).toBe("public");
  fireEvent.click(screen.getByRole("button", { name: /FAILED · 1 attempts/ }));
  expect(await screen.findByText("Failure: USER_CODE")).toBeTruthy();
  expect(calls.find((c) => c.query.plan)?.query.branch).toBe("public");
  model.dispose();
});
test("branch failures stay visible even when published data falls back elsewhere", async () => {
  const { model, calls } = await fixture();
  await model.select({
    branch: "review",
    dataset: datasetId,
    origin: workspaceId,
  });
  const state = model.snapshot();
  if (state.kind !== "ready") throw new Error("fixture not ready");
  render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  expect(await screen.findByText("100.00%")).toBeTruthy();
  expect(screen.getByTitle("History branch").textContent).toContain("review");
  expect(
    screen.queryByText(/Could not find data on current branch/),
  ).toBeNull();
  expect(
    calls
      .filter((c) => c.path.endsWith("/history") || c.path.endsWith("/metrics"))
      .every((c) => c.query.branch === "review"),
  ).toBe(true);
  model.dispose();
});
test("history summary uses compact minute timestamps, rounded durations and concise point tooltips", async () => {
  const duration = "2564999999";
  const { model, state } = await fixture(
    false,
    {
      ...job,
      state: "SUCCEEDED",
      build_state: "SUCCEEDED",
      duration_ns: duration,
      attempts: job.attempts.map((timing) => ({
        ...timing,
        state: "SUCCEEDED",
        duration_ns: duration,
      })),
    },
    {
      metrics: {
        build_states: [{ state: "SUCCEEDED", count: "1" }],
        job_states: [{ state: "SUCCEEDED", count: "1" }],
        materializations: "1",
        failure_rate: { numerator: "0", denominator: "1" },
        duration_samples: "1",
        median_ns: { numerator: duration, denominator: "1" },
        trailing_mean_ns: { numerator: duration, denominator: "1" },
        trailing_samples: "1",
      },
    },
  );
  const { container } = render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  expect(await screen.findByText("0.00%")).toBeTruthy();
  expect(screen.getByTitle("2023-11-14 22:13").textContent).toBe(
    "2023-11-14 22:13",
  );
  expect(screen.getByTitle("Job duration").textContent).toBe("(2.56 s)");
  const heading = screen.getByRole("heading", { name: "Summary" });
  expect(
    heading.parentElement?.contains(
      screen.getByRole("textbox", { name: "From" }),
    ),
  ).toBe(true);
  expect(
    Array.from(
      container.querySelectorAll(".metric small"),
      (el) => el.textContent,
    ),
  ).toEqual([
    "Builds",
    "Failure rate",
    "Mean · latest 10 successes",
    "Median duration",
  ]);
  fireEvent.change(screen.getByRole("textbox", { name: "From" }), {
    target: { value: "2023-11-14" },
  });
  fireEvent.change(screen.getByRole("textbox", { name: "To" }), {
    target: { value: "2023-11-14" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Apply" }));
  const chart = await screen.findByRole("img", {
    name: "Measured attempt durations by build acceptance time",
  });
  const tooltip = chart.querySelector("circle title")?.textContent;
  expect(tooltip).toBe("SUCCEEDED · 2023-11-14 22:13 · 2.56 s");
  expect(tooltip).not.toContain(" ns");
  expect(tooltip).not.toContain(attempt);
  expect(chart.textContent).not.toContain("2.565");
  model.dispose();
});
test("Gantt uses recorded phases and exposes retained failure evidence in the owning plan", async () => {
  const { model, state, calls } = await fixture();
  render(<TimelineInspector workspace={model} state={state} />);
  fireEvent.click(
    await screen.findByRole("button", { name: "View build report" }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Progress details" }),
  );
  const bar = await screen.findByRole("button", {
    name: /attempt 1: Running, 1.00 s/,
  });
  expect(bar.getAttribute("style")).toContain("width: 100%");
  fireEvent.click(screen.getByRole("button", { name: /Critical path/ }));
  expect(screen.getByText("Most critical")).toBeTruthy();
  fireEvent.click(
    await screen.findByRole("button", { name: /attempt 1: FAILED/ }),
  );
  expect(await screen.findByText("Failure: USER_CODE")).toBeTruthy();
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

test("job overview opens a modal, preserves selection and hides manual schedule card", async () => {
  const { model, state } = await fixture();
  render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: /FAILED · 1 attempts/ }),
  );
  expect(await screen.findByText("Transaction details")).toBeTruthy();
  expect(screen.getByText("No version published")).toBeTruthy();
  const opener = screen.getByRole("button", { name: "View build report" });
  opener.focus();
  fireEvent.click(opener);
  const dialog = screen.getByRole("dialog", { name: "Build report" });
  expect(within(dialog).getByText("Build info")).toBeTruthy();
  expect(within(dialog).queryByText("Build schedule")).toBeNull();
  expect(within(dialog).queryByText("Targets and provenance")).toBeNull();
  expect(within(dialog).getAllByText("Job status")).toHaveLength(1);
  expect(
    within(dialog).getByRole("button", {
      name: "Filter by job status: All statuses",
    }),
  ).toBeTruthy();
  expect(
    within(dialog).getByRole("button", { name: "Refresh" }).textContent,
  ).toBe("");
  expect(
    within(dialog).getByRole("button", { name: "Critical path" }),
  ).toHaveProperty("disabled", false);
  const filter = within(dialog).getByRole("button", {
    name: "Filter by job status: All statuses",
  });
  fireEvent.click(filter);
  fireEvent.keyDown(
    screen.getByRole("textbox", { name: "Search job statuses" }),
    { key: "Escape" },
  );
  expect(screen.getByRole("dialog", { name: "Build report" })).toBe(dialog);
  expect(
    screen.queryByRole("dialog", { name: "Job status filter" }),
  ).toBeNull();
  expect(document.activeElement).toBe(filter);
  fireEvent(dialog, new Event("cancel"));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(opener);
  expect(screen.getByText("Transaction details")).toBeTruthy();
  model.dispose();
});

test("cached Build report displays completion as start, no execution queue wait and a historical estimate", async () => {
  const cached = {
    ...job,
    state: "CACHED",
    build_state: "SUCCEEDED",
    attempt_count: "0",
    attempts: [],
    duration_ns: null,
    reused_version: versionId,
  };
  const { model, state } = await fixture(false, cached, {
    timeline: () => ({
      state: "SUCCEEDED",
      started_us: null,
      initial_queue_wait_us: null,
      duration_estimate: {
        window: "10",
        samples: "2",
        missing_samples: "0",
        mean_us: { numerator: "1500000", denominator: "2" },
      },
      job_counts: [{ state: "CACHED", count: "1" }],
      jobs: [
        {
          job: jobId,
          dataset: datasetId,
          state: "CACHED",
          duration_ns: null,
          attempts: [],
        },
      ],
    }),
  });
  render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: /CACHED · Reuse/ }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "View build report" }),
  );
  const dialog = screen.getByRole("dialog", { name: "Build report" });
  expect(await within(dialog).findByText("~ 0.750 s")).toBeTruthy();
  const started = within(dialog).getByText("Started").nextElementSibling;
  const ended = within(dialog).getByText("Ended").nextElementSibling;
  expect(started?.textContent).toBe(ended?.textContent);
  expect(started?.getAttribute("title")).toContain("no execution started");
  expect(
    within(dialog).getByLabelText("Not applicable: no execution queued")
      .textContent,
  ).toBe("—");
  expect(dialog.querySelectorAll(".phase-bar")).toHaveLength(0);
  expect(
    within(dialog).getByTitle(/Mean total duration of 2 previous/),
  ).toBeTruthy();
  model.dispose();
});

test("a noncached Build report keeps unknown execution start and queue wait unavailable", async () => {
  const pending = {
    ...job,
    state: "QUEUED",
    build_state: "QUEUED",
    attempt_count: "0",
    attempts: [],
    duration_ns: null,
  };
  const { model, state } = await fixture(false, pending, {
    timeline: () => ({
      state: "QUEUED",
      started_us: null,
      finished_us: null,
      initial_queue_wait_us: null,
      job_counts: [{ state: "QUEUED", count: "1" }],
      jobs: [
        {
          job: jobId,
          dataset: datasetId,
          state: "QUEUED",
          duration_ns: null,
          attempts: [],
        },
      ],
    }),
  });
  const view = render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: /QUEUED · 0 attempts/ }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "View build report" }),
  );
  const dialog = screen.getByRole("dialog", { name: "Build report" });
  await within(dialog).findByText("Build info");
  expect(
    within(dialog).getByText("Started").nextElementSibling?.textContent,
  ).toBe("Unavailable");
  expect(
    within(dialog).getByText("Queue wait").nextElementSibling?.textContent,
  ).toBe("Unavailable");
  expect(
    within(dialog).queryByLabelText("Not applicable: no execution queued"),
  ).toBeNull();
  view.unmount();
  model.dispose();
});

test("active evidence refreshes after errors, stops at completion and aborts on unmount", async () => {
  const control = {
    fail: false,
    timeline: (): Partial<ApiExecutionTimelineV1> => ({
      state: "RUNNING",
      finished_us: null,
      wall_duration_us: null,
      critical_path: null,
    }),
  };
  const { model, state, calls } = await fixture(
    false,
    { ...job, build_state: "RUNNING" },
    control,
  );
  vi.useFakeTimers();
  try {
    const hook = renderHook(() =>
      useBuildEvidence(model, state, {
        ...job,
        build_state: "RUNNING",
      }),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(hook.result.current.result?.timeline.state).toBe("RUNNING");
    control.fail = true;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(hook.result.current.error).toContain("disconnected");
    control.fail = false;
    control.timeline = () => ({ state: "SUCCEEDED" });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    expect(hook.result.current.result?.timeline.state).toBe("SUCCEEDED");
    expect(hook.result.current.error).toBe("");
    const completed = calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6000);
    });
    expect(calls.length).toBe(completed);
    hook.unmount();
    control.timeline = () => ({ state: "RUNNING", finished_us: null });
    const second = renderHook(() =>
      useBuildEvidence(model, state, {
        ...job,
        build_state: "RUNNING",
      }),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    second.unmount();
    const closed = calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6000);
    });
    expect(calls.length).toBe(closed);
  } finally {
    vi.useRealTimers();
    model.dispose();
  }
});

test("Gantt orders started jobs first, updates open spans and freezes completed evidence", async () => {
  const { model, state } = await fixture();
  const scope = await model.scope(
    { branch: state.selection.branch, plan },
    new AbortController().signal,
  );
  const base = await scope.read(
    "ApiExecutionTimelineV1",
    `/api/v1/builds/${build}/timeline`,
  );
  const one = base.jobs[0];
  if (!one) throw new Error("Missing fixture job");
  const firstAttempt = one.attempts[0];
  if (!firstAttempt) throw new Error("Missing fixture attempt");
  const later = {
    ...one,
    job: "later",
    path: "later",
    attempts: [
      {
        ...firstAttempt,
        started_us: "1700000002000000",
        finished_us: null,
      },
    ],
  };
  const early = { ...one, job: "early", path: "early" };
  const waiting = { ...one, job: "waiting", path: "waiting", attempts: [] };
  const timeline = {
    ...base,
    state: "RUNNING",
    finished_us: null,
    queued_us: "1699999900000000",
    jobs: [later, waiting, early],
  };
  expect(orderedJobs(timeline.jobs).map((j) => j.job)).toEqual([
    "early",
    "later",
    "waiting",
  ]);
  const view = render(
    <BuildGantt
      timeline={timeline}
      mode="status"
      now={1700000004000000n}
      name={(j) => j.path ?? j.job}
      onAttempt={() => {}}
    />,
  );
  expect(
    [...view.container.querySelectorAll(".gantt-label strong")].map(
      (e) => e.textContent,
    ),
  ).toEqual(["early", "later", "waiting"]);
  const bar = screen.getByRole("button", { name: /later attempt/ });
  expect(bar.style.width).toBe("50%");
  expect(screen.getByRole("button", { name: /early attempt/ }).style.left).toBe(
    "0%",
  );
  expect(view.container.querySelector(".gantt-label small")).toBeNull();
  expect(bar.textContent).toBe("");
  expect(bar.title).toContain("2.00 s elapsed");
  expect(screen.getByRole("button", { name: /early attempt/ }).title).toMatch(
    / · 1\.00 s$/,
  );
  view.rerender(
    <BuildGantt
      timeline={timeline}
      mode="status"
      now={1700000006000000n}
      name={(j) => j.path ?? j.job}
      onAttempt={() => {}}
    />,
  );
  expect(parseFloat(bar.style.width)).toBeGreaterThan(66);
  expect(bar.title).toContain("4.00 s elapsed");
  model.dispose();
});

test.each([false, true])(
  "scheduled active modal exposes retained trigger with manual request %s and disables critical path",
  async (manual) => {
    const { model, state } = await fixture(false, job, {
      trigger: {
        kind: "schedule",
        name: "Daily fixture",
        schedule_id: "fixture-schedule",
        manual,
      },
      timeline: () => ({
        state: "RUNNING",
        finished_us: null,
        critical_path: null,
      }),
    });
    const scope = await model.scope(
      { branch: state.selection.branch, plan },
      new AbortController().signal,
    );
    const timeline = await scope.read(
      "ApiExecutionTimelineV1",
      `/api/v1/builds/${build}/timeline`,
    );
    const report = await scope.read(
      "ExecutionJsonV1",
      `/api/v1/builds/${build}`,
    );
    const view = render(
      <BuildModal
        result={{ timeline, report, scope }}
        error=""
        refresh={() => {}}
        dataset={
          state.value.dataset ??
          (() => {
            throw new Error("Missing dataset");
          })()
        }
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("Build schedule")).toBeTruthy();
    expect(screen.getByText("Daily fixture")).toBeTruthy();
    expect(
      screen.getByText(
        manual
          ? "Manually requested scheduled build"
          : "Scheduled build from accepted occurrence evidence",
      ),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Critical path" }),
    ).toHaveProperty("disabled", true);
    expect(screen.getByText("Live · updates automatically")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox", { name: "Dataset path" }), {
      target: { value: "no-such-dataset" },
    });
    expect(screen.getByText("No jobs match these filters.")).toBeTruthy();
    view.unmount();
    model.dispose();
  },
);

test("job transaction metadata stays pinned to its own version without changing Preview", async () => {
  const { model, state, calls } = await fixture(false, {
    ...job,
    state: "SUCCEEDED",
    produced_version: versionId,
  });
  render(
    <HistoryInspector
      workspace={model}
      state={state}
      onVersion={() => {
        throw new Error("Must not switch Preview");
      }}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: /SUCCEEDED · 1 attempts/ }),
  );
  expect(await screen.findByText("0.04 KB")).toBeTruthy();
  expect(
    calls.some(
      (c) =>
        c.path.endsWith("/versions") &&
        c.query.version === versionId &&
        c.query.dataset === datasetId,
    ),
  ).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Files" }));
  expect(screen.getByText("a".repeat(64))).toBeTruthy();
  model.dispose();
});

test("mismatched build timeline is rejected instead of painting a different build", async () => {
  const { model, state } = await fixture(false, job, {
    timeline: () => ({ build: "55555555-5555-4555-8555-555555555555" }),
  });
  const hook = renderHook(() => useBuildEvidence(model, state, job));
  await waitFor(() =>
    expect(hook.result.current.error).toContain("does not match"),
  );
  expect(hook.result.current.result).toBeUndefined();
  hook.unmount();
  model.dispose();
});

test("background metadata refresh keeps the selected job and Build modal mounted", async () => {
  const { model } = await fixture();
  render(<App workspace={model} />);
  fireEvent.click(screen.getByRole("button", { name: "Catalogue" }));
  fireEvent.click(
    await screen.findByRole("button", { name: /raw\/example, local/ }),
  );
  await screen.findByRole("button", { name: "1 node selected" });
  fireEvent.click(screen.getByRole("tab", { name: "History" }));
  fireEvent.click(
    await screen.findByRole("button", { name: /FAILED · 1 attempts/ }),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "View build report" }),
  );
  const dialog = screen.getByRole("dialog");
  await act(async () => {
    await model.refresh(true);
  });
  expect(screen.getByRole("dialog")).toBe(dialog);
  expect(
    screen
      .getByRole("button", { name: /FAILED · 1 attempts/ })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  model.dispose();
});

test("History chart excludes failed and cached attempts while retaining failure metrics and job entries", async () => {
  const success = {
    ...job,
    id: versionId,
    state: "SUCCEEDED",
    attempts: job.attempts.map((a) => ({
      ...a,
      attempt: versionId,
      state: "SUCCEEDED",
      duration_ns: "2000000000",
    })),
  };
  const cached = { ...job, id: workspaceId, state: "CACHED", attempts: [] };
  const { model, state } = await fixture(false, job, {
    jobs: [job, success, cached],
  });
  const view = render(
    <HistoryInspector workspace={model} state={state} onVersion={() => {}} />,
  );
  fireEvent.change(screen.getByRole("textbox", { name: "From" }), {
    target: { value: "2023-11-14" },
  });
  fireEvent.change(screen.getByRole("textbox", { name: "To" }), {
    target: { value: "2023-11-14" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Apply" }));
  const chart = await screen.findByRole("img", {
    name: "Measured attempt durations by build acceptance time",
  });
  expect(chart.querySelectorAll("circle")).toHaveLength(1);
  expect(chart.querySelector("circle title")?.textContent).toContain(
    "SUCCEEDED",
  );
  expect(chart.querySelector(".status-failed")).toBeNull();
  expect(view.container.querySelectorAll(".history-entry")).toHaveLength(3);
  expect(screen.getByText("100.00%")).toBeTruthy();
  model.dispose();
});

test("Python factories always show the definition in saved, published and diff views", async () => {
  const { model, state, calls } = await fixture(false, job, { factory: true });
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText("src/factory.py")).toBeTruthy();
  expect(
    screen.getAllByRole("button").map((button) => button.textContent),
  ).toEqual([
    "Authored",
    "Generated",
    "Latest saved version",
    "Data publish version",
    "Diff",
  ]);
  expect(
    screen.queryByRole("navigation", { name: "SQL source views" }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Data publish version" }));
  expect(await screen.findByText("# producing")).toBeTruthy();
  expect(await screen.findByText("src/factory.py")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Diff" }));
  expect(
    await screen.findByRole("region", {
      name: "Source diff: producing to current retained definition",
    }),
  ).toBeTruthy();
  expect(
    calls
      .filter((call) => call.path.includes("/source/"))
      .every((call) => call.query.path === "src/factory.py"),
  ).toBe(true);
  model.dispose();
});

test("Python Generated reads the matching saved or published preview and keeps Diff authored", async () => {
  const { model, state } = await fixture(false, job, {
    factory: true,
    pythonPreview: true,
  });
  render(<CodeInspector workspace={model} state={state} />);
  fireEvent.click(screen.getByRole("button", { name: /^Generated$/ }));
  expect(await screen.findByText("# Saved preview")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Data publish version" }));
  expect(await screen.findByText("# Published preview")).toBeTruthy();
  expect(screen.queryByText("# Saved preview")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Diff" }));
  expect(
    await screen.findByRole("region", {
      name: "Source diff: producing to current retained definition",
    }),
  ).toBeTruthy();
  expect(
    screen
      .getByRole("button", { name: /^Authored$/ })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: /^Generated$/ }));
  expect(await screen.findByText("# Saved preview")).toBeTruthy();
  expect(
    screen
      .getByRole("button", { name: "Latest saved version" })
      .getAttribute("aria-pressed"),
  ).toBe("true");
  model.dispose();
});

test("Python Generated remains available when the authored file is a bounded excerpt", async () => {
  const { model, state } = await fixture(false, job, {
    factory: true,
    pythonPreview: true,
    boundedSource: true,
  });
  render(<CodeInspector workspace={model} state={state} />);
  expect(await screen.findByText(/Showing a bounded excerpt/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /^Generated$/ }));
  expect(
    await screen.findByRole("textbox", {
      name: "Read-only generated Python preview",
    }),
  ).toBeTruthy();
  expect(screen.getByText("# Saved preview")).toBeTruthy();
  model.dispose();
});

test("Python Generated explains missing retained previews without fabricating source", async () => {
  const { model, state } = await fixture(false, job, { factory: true });
  render(<CodeInspector workspace={model} state={state} />);
  fireEvent.click(screen.getByRole("button", { name: /^Generated$/ }));
  expect(
    await screen.findByText(/No expanded Python preview is retained/),
  ).toBeTruthy();
  expect(
    screen.queryByRole("textbox", {
      name: "Read-only generated Python preview",
    }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /^Authored$/ }));
  expect(await screen.findByText("# current")).toBeTruthy();
  model.dispose();
});

test("SQL Code shows actual attempt SQL and never substitutes a failed generator's older SQL", async () => {
  const { model, state, calls } = await fixture(false, job, {
    factorySql: true,
    generatedSql: true,
  });
  if (!state.value.dataset) throw new Error("Missing dataset fixture");
  const sqlState: typeof state = {
    ...state,
    value: {
      ...state.value,
      dataset: {
        ...state.value.dataset,
        producer: {
          engine: "duckdb",
          authoring: { kind: "sql", definition_path: "src/factory.py" },
        },
      },
    },
  };
  render(<CodeInspector workspace={model} state={sqlState} />);
  expect(await screen.findByText("# current")).toBeTruthy();
  const header = screen
    .getByRole("heading", { name: state.value.dataset.path })
    .closest(".dataset-inspector-header");
  expect(header?.contains(screen.getByText("Dynamically generated code"))).toBe(
    true,
  );
  expect(
    header?.contains(screen.getByRole("button", { name: /^Authored$/ })),
  ).toBe(true);
  expect(
    header?.contains(screen.getByRole("button", { name: /^Generated$/ })),
  ).toBe(true);
  expect(
    screen.queryByRole("button", { name: "Factory definition" }),
  ).toBeNull();
  expect(screen.queryByRole("button", { name: "Registration" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /^Generated$/ }));
  expect(
    await screen.findByRole("textbox", { name: "Read-only generated SQL" }),
  ).toBeTruthy();
  expect(
    screen.queryByRole("navigation", { name: "Factory source views" }),
  ).toBeNull();
  expect(
    screen.queryByText(/Latest executed SQL; saved code may have changed/),
  ).toBeNull();
  expect(screen.queryByText(new RegExp(`Attempt ${attempt}`))).toBeNull();
  expect(
    calls.find((call) => call.path.endsWith(`/attempts/${attempt}`))?.query
      .plan,
  ).toBe(plan);
  model.dispose();
});

test("SQL Code explicitly reports missing generation evidence for the latest failure", async () => {
  const { model, state } = await fixture(false, job, { factorySql: true });
  if (!state.value.dataset) throw new Error("Missing dataset fixture");
  const sqlState: typeof state = {
    ...state,
    value: {
      ...state.value,
      dataset: { ...state.value.dataset, producer: { engine: "duckdb" } },
    },
  };
  render(<CodeInspector workspace={model} state={sqlState} />);
  fireEvent.click(screen.getByRole("button", { name: /^Generated$/ }));
  expect(
    await screen.findByText(/SQL was not generated for this attempt/),
  ).toBeTruthy();
  expect(
    screen.queryByRole("textbox", { name: "Read-only generated SQL" }),
  ).toBeNull();
  model.dispose();
});
