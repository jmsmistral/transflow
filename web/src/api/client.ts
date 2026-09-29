import schema from "../../../schemas/contracts-v1.schema.json";
import type {
  ApiCapabilitiesV1,
  ApiContextV1,
  ApiDatasetV1,
  ApiDatasetsV1,
  ApiMetadataPageV1,
  ApiVersionsV1,
  ApiSourceV1,
  ApiPreviewV1,
  ApiSessionV1,
  ApiVerifiedV1,
  ApiEventV1,
  ApiLineageV1,
  GraphViewV1,
  ApiViewsV1,
} from "../generated/contracts";
import { obj, same, validate } from "./validate";

export interface Contracts {
  GraphViewV1: GraphViewV1;
  ApiViewsV1: ApiViewsV1;
  ApiLineageV1: ApiLineageV1;
  ApiCapabilitiesV1: ApiCapabilitiesV1;
  ApiContextV1: ApiContextV1;
  ApiDatasetV1: ApiDatasetV1;
  ApiDatasetsV1: ApiDatasetsV1;
  ApiMetadataPageV1: ApiMetadataPageV1;
  ApiVersionsV1: ApiVersionsV1;
  ApiSourceV1: ApiSourceV1;
  ApiPreviewV1: ApiPreviewV1;
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
