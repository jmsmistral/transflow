import schema from "../../../schemas/contracts-v1.schema.json";
import type {
  ApiSelectionV1,
  ApiAcceptedV1,
  PlanResultV1,
  ApiAttemptV1,
  ApiAttemptLogsV1,
  ApiExecutionTimelineV1,
  ApiExecutionMetricsV1,
  ApiDatasetHistoryV1,
  ApiCanceledV1,
  ExecutionJsonV1,
  ApiCapabilitiesV1,
  ApiContextV1,
  ApiDatasetV1,
  ApiDatasetInspectionV1,
  ApiDatasetsV1,
  ApiMetadataPageV1,
  ApiVersionsV1,
  ApiSourceV1,
  ApiPreviewV1,
  ApiPreviewRequestV1,
  LogicalSchemaV1,
  ApiSessionV1,
  ApiVerifiedV1,
  ApiEventV1,
  ApiLineageV1,
  GraphViewV1,
  ApiViewsV1,
} from "../generated/contracts";
import { obj, same, validate } from "./validate";

export interface Contracts {
  ApiSelectionV1: ApiSelectionV1;
  PlanResultV1: PlanResultV1;
  ApiAcceptedV1: ApiAcceptedV1;
  ApiAttemptV1: ApiAttemptV1;
  ApiAttemptLogsV1: ApiAttemptLogsV1;
  ApiExecutionTimelineV1: ApiExecutionTimelineV1;
  ApiExecutionMetricsV1: ApiExecutionMetricsV1;
  ApiDatasetHistoryV1: ApiDatasetHistoryV1;
  ApiCanceledV1: ApiCanceledV1;
  ExecutionJsonV1: ExecutionJsonV1;

  GraphViewV1: GraphViewV1;
  ApiViewsV1: ApiViewsV1;
  ApiLineageV1: ApiLineageV1;
  ApiCapabilitiesV1: ApiCapabilitiesV1;
  ApiContextV1: ApiContextV1;
  ApiDatasetV1: ApiDatasetV1;
  ApiDatasetInspectionV1: ApiDatasetInspectionV1;
  ApiDatasetsV1: ApiDatasetsV1;
  ApiMetadataPageV1: ApiMetadataPageV1;
  ApiVersionsV1: ApiVersionsV1;
  ApiSourceV1: ApiSourceV1;
  ApiPreviewV1: ApiPreviewV1;
  LogicalSchemaV1: LogicalSchemaV1;
  ApiSessionV1: ApiSessionV1;
  ApiVerifiedV1: ApiVerifiedV1;
  ApiEventV1: ApiEventV1;
}
const definitions = obj(schema.$defs);
/** The sole assertion boundary: Rust-owned schemas validate unknown network data before use. */
export function decode<K extends keyof Contracts>(
  name: K,
  value: unknown,
): Contracts[K] {
  validate(obj(definitions[name]), value, definitions);
  return value as Contracts[K];
}
export class ApiFailure extends Error {
  constructor(
    readonly kind: "disconnected" | "expired" | "conflict" | "failed",
    message: string,
  ) {
    super(message);
  }
}
export type Query = Readonly<Record<string, string>>;
export interface Envelope<T> {
  context: ApiContextV1 | null;
  data: T;
}

/** Same-origin session only. Credentials live in memory and an HttpOnly cookie, never storage. */
export class Client {
  private csrf = "";
  constructor(
    private readonly transport: typeof fetch = (...args) => fetch(...args),
  ) {}
  async connect(code: string, signal: AbortSignal): Promise<ApiCapabilitiesV1> {
    if (!code)
      throw new ApiFailure(
        "expired",
        "Open a fresh launch link from transflow serve --open.",
      );
    const session = await this.post(
      "/api/v1/sessions/exchange",
      { code },
      signal,
    );
    signal.throwIfAborted();
    this.csrf = decode("ApiSessionV1", session.data).csrf;
    const verified = await this.post("/api/v1/sessions/verify", {}, signal);
    decode("ApiVerifiedV1", verified.data);
    return (
      await this.read("ApiCapabilitiesV1", "/api/v1/capabilities", {}, signal)
    ).data;
  }
  async read<K extends keyof Contracts>(
    name: K,
    path: string,
    query: Query,
    signal: AbortSignal,
    context?: ApiContextV1,
  ): Promise<Envelope<Contracts[K]>> {
    const value = await this.post(
      "/api/v1/read",
      {
        path,
        query: {
          ...query,
          ...(context ? { context: context.fingerprint } : {}),
        },
      },
      signal,
    );
    return this.envelope(name, value, context);
  }
  private envelope<K extends keyof Contracts>(
    name: K,
    value: Record<string, unknown>,
    expected?: ApiContextV1,
  ): Envelope<Contracts[K]> {
    const context =
      value.context === null ? null : decode("ApiContextV1", value.context);
    if (
      expected &&
      (!context ||
        context.fingerprint !== expected.fingerprint ||
        !same(context, expected))
    ) {
      throw new ApiFailure(
        "conflict",
        "The selected context changed. Refresh before continuing.",
      );
    }
    return { context, data: decode(name, value.data) };
  }
  async saveView(
    view: GraphViewV1,
    query: Query,
    context: ApiContextV1,
    key: string,
    signal: AbortSignal,
  ): Promise<GraphViewV1> {
    const url = `/api/v1/views?${new URLSearchParams({ ...query, context: context.fingerprint })}`;
    const value = await this.post(url, view, signal, {
      "If-Match": `"${context.fingerprint}"`,
      "Idempotency-Key": key,
    });
    return this.envelope("GraphViewV1", value, context).data;
  }
  async preparePlan(
    request: ApiSelectionV1,
    context: ApiContextV1,
    key: string,
    signal: AbortSignal,
  ) {
    if (
      request.branch !== context.branch ||
      context.selection.kind !== "retained_current"
    )
      throw new ApiFailure(
        "conflict",
        "Choose a current branch context before planning.",
      );
    const query = {
      branch: request.branch,
      fallback: JSON.stringify(context.fallback_policy.slice(1)),
    };
    const value = await this.post(
      `/api/v1/plans?${new URLSearchParams({ ...query, context: context.fingerprint })}`,
      request,
      signal,
      {
        "If-Match": `"${context.fingerprint}"`,
        "Idempotency-Key": key,
      },
    );
    const plan = this.envelope("PlanResultV1", value, context).data;
    if (plan.workspace !== context.workspace || plan.branch !== request.branch)
      throw new ApiFailure(
        "conflict",
        "The preview does not match this workspace and branch.",
      );
    const selected = await this.read(
      "ApiContextV1",
      "/api/v1/context",
      { branch: plan.branch, plan: plan.plan_id },
      signal,
    );
    if (
      !selected.context ||
      !same(selected.context, selected.data) ||
      selected.data.workspace !== plan.workspace ||
      selected.data.branch !== plan.branch ||
      selected.data.source !== plan.source ||
      selected.data.selection.kind !== "plan" ||
      selected.data.selection.id !== plan.plan_id ||
      selected.data.selection.digest !== plan.digest
    )
      throw new ApiFailure(
        "conflict",
        "The preview's retained plan context does not match.",
      );
    return { plan, context: selected.data };
  }
  async acceptPlan(
    plan: PlanResultV1,
    context: ApiContextV1,
    key: string,
    signal: AbortSignal,
  ): Promise<ApiAcceptedV1> {
    if (
      context.selection.kind !== "plan" ||
      context.selection.id !== plan.plan_id ||
      context.selection.digest !== plan.digest ||
      context.workspace !== plan.workspace ||
      context.branch !== plan.branch
    )
      throw new ApiFailure(
        "conflict",
        "Preview this plan again before building.",
      );
    const value = await this.post(
      `/api/v1/builds?${new URLSearchParams({ branch: plan.branch, plan: plan.plan_id, context: context.fingerprint })}`,
      { kind: "plan", plan_id: plan.plan_id },
      signal,
      {
        "If-Match": `"${context.fingerprint}"`,
        "Idempotency-Key": key,
      },
    );
    const accepted = this.envelope("ApiAcceptedV1", value, context).data;
    if (accepted.plan !== plan.plan_id)
      throw new ApiFailure(
        "conflict",
        "The accepted build does not match the preview.",
      );
    return accepted;
  }
  async cancelBuild(
    build: string,
    query: Query,
    context: ApiContextV1,
    key: string,
    signal: AbortSignal,
  ): Promise<Contracts["ApiCanceledV1"]> {
    const value = await this.post(
      `/api/v1/builds/${encodeURIComponent(build)}/cancel?${new URLSearchParams({ ...query, context: context.fingerprint })}`,
      {},
      signal,
      { "If-Match": `"${context.fingerprint}"`, "Idempotency-Key": key },
    );
    const result = decode("ApiCanceledV1", value.data);
    if (result.build !== build)
      throw new ApiFailure(
        "conflict",
        "Cancellation does not match this build.",
      );
    return result;
  }
  async preview(
    request: ApiPreviewRequestV1,
    query: Query,
    context: ApiContextV1,
    signal: AbortSignal,
  ): Promise<ApiPreviewV1> {
    const url = `/api/v1/previews?${new URLSearchParams({ ...query, context: context.fingerprint })}`;
    const value = await this.post(url, request, signal);
    const responseContext = decode("ApiContextV1", value.context);
    const data = decode("ApiPreviewV1", value.data);
    if (
      responseContext.workspace !== context.workspace ||
      responseContext.branch !== context.branch ||
      responseContext.registry !== context.registry ||
      responseContext.configuration !== context.configuration ||
      responseContext.runtime_revision !== context.runtime_revision ||
      !same(responseContext.fallback_policy, context.fallback_policy) ||
      responseContext.selection.id !== request.version ||
      responseContext.selection.kind !==
        (request.origin_workspace === context.workspace
          ? "local_version"
          : "foreign_version") ||
      data.workspace !== context.workspace ||
      data.requested_branch !== context.branch ||
      data.dataset !== request.dataset ||
      data.origin_workspace !== request.origin_workspace ||
      data.version !== request.version
    )
      throw new ApiFailure(
        "conflict",
        "The preview does not match the selected version.",
      );
    return data;
  }
  async response(
    path: string,
    body: unknown,
    signal: AbortSignal,
    headers: Record<string, string> = {},
  ): Promise<Response> {
    let response: Response;
    try {
      response = await this.transport(path, {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        redirect: "error",
        headers: {
          ...headers,
          "Content-Type": "application/json",
          ...(this.csrf ? { "X-Transflow-CSRF": this.csrf } : {}),
        },
        body: JSON.stringify(body),
        signal,
      });
    } catch (error) {
      if (signal.aborted) throw error;
      throw new ApiFailure(
        "disconnected",
        "The coordinator is disconnected. Check that it is running, then reconnect.",
      );
    }
    if (!response.ok)
      throw new ApiFailure(
        response.status === 401
          ? "expired"
          : response.status === 409
            ? "conflict"
            : "failed",
        response.status === 401
          ? "This session expired. Open a fresh launch link from transflow serve --open."
          : response.status === 409
            ? "The selected context changed. Refresh before continuing."
            : "The request failed. Refresh or inspect the coordinator diagnostics.",
      );
    return response;
  }
  private async post(
    path: string,
    body: unknown,
    signal: AbortSignal,
    headers: Record<string, string> = {},
  ): Promise<Record<string, unknown>> {
    const response = await this.response(path, body, signal, headers);
    if (!response.headers.get("content-type")?.startsWith("application/json"))
      throw new ApiFailure(
        "failed",
        "The coordinator returned an unsupported response.",
      );
    if (!response.body)
      throw new ApiFailure("failed", "The coordinator returned no response.");
    const reader = response.body.getReader();
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      while (true) {
        const { done, value } = await readChunk(reader, signal);
        if (done) break;
        size += value.byteLength;
        if (size > 32 * 1024 * 1024)
          throw new ApiFailure(
            "failed",
            "The response exceeded the supported size.",
          );
        chunks.push(value);
      }
    } finally {
      // A failed stream may also reject cancellation; preserve its original diagnostic.
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    return obj(
      JSON.parse(
        new TextDecoder("utf-8", { fatal: true }).decode(bytes),
      ) as unknown,
    );
  }
}

/** Body transport errors are disconnected states, distinct from invalid JSON/schema content. */
export async function readChunk(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  signal: AbortSignal,
): Promise<ReadableStreamReadResult<Uint8Array>> {
  try {
    return await reader.read();
  } catch (error) {
    if (signal.aborted) throw error;
    throw new ApiFailure(
      "disconnected",
      "The coordinator disconnected while sending data. Check that it is running, then reconnect.",
    );
  }
}
