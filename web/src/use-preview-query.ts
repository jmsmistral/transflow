import { useEffect, useRef, useState } from "react";
import type {
  ApiContextV1,
  ApiQueryRequestV1,
  ApiQueryResultsV1,
} from "./generated/contracts";
import type { Workspace } from "./workspace";
import { same } from "./api/validate";

type Result = {
  key: string;
  page: ApiQueryResultsV1 | null;
  loading: boolean;
  running: boolean;
  error: string;
};
type Owned = {
  closed: boolean;
  canceled: boolean;
  running: boolean;
  paging: boolean;
  abort: AbortController;
  timer?: ReturnType<typeof setTimeout>;
  query?: { id: string; context: ApiContextV1 };
  page: ApiQueryResultsV1 | null;
  more?: () => void;
};

/** Own one exact-version query, including late acceptance and selection cleanup. */
export function usePreviewQuery(
  workspace: Workspace,
  context: ApiContextV1 | null,
  request: ApiQueryRequestV1 | null,
) {
  const key = JSON.stringify([context?.fingerprint, request]);
  const [result, setResult] = useState<Result | null>(null);
  const [revision, setRevision] = useState(0);
  const owned = useRef<Owned | null>(null);
  useEffect(() => {
    if (!context || !request) return;
    const lifetime: Owned = {
      closed: false,
      canceled: false,
      running: true,
      paging: false,
      abort: new AbortController(),
      page: null,
    };
    owned.current = lifetime;
    const alive = () => !lifetime.closed && !lifetime.canceled;
    const update = (patch: Partial<Result>) => {
      if (alive())
        setResult((old) => ({
          key,
          page: lifetime.page,
          loading: false,
          running: lifetime.running,
          error: "",
          ...(old?.key === key ? old : {}),
          ...patch,
        }));
    };
    const cancelOwned = () => {
      if (lifetime.query && lifetime.running) {
        const { id, context: acceptedContext } = lifetime.query;
        // A lost connection leaves the helper subject to its server deadline.
        void workspace.client
          .cancelQuery(id, acceptedContext, new AbortController().signal)
          .catch(() => undefined);
      }
    };
    const fail = (error: unknown) => {
      if (!alive()) return;
      clearTimeout(lifetime.timer);
      cancelOwned();
      update({
        loading: false,
        running: false,
        error: error instanceof Error ? error.message : "Preview query failed.",
      });
    };
    async function page(offset = 0) {
      const query = lifetime.query;
      if (!query || !alive() || lifetime.paging) return;
      lifetime.paging = true;
      update({ loading: true });
      try {
        const response = (
          await workspace.client.read(
            "ApiQueryResultsV1",
            `/api/v1/queries/${query.id}/results`,
            {
              branch: query.context.branch,
              offset: String(offset),
              limit: "500",
            },
            lifetime.abort.signal,
            query.context,
          )
        ).data;
        const previous = lifetime.page;
        if (
          response.id !== query.id ||
          response.rows.length > 500 ||
          response.rows.some(
            (r) => r.length !== response.schema.fields.length,
          ) ||
          (offset > 0 &&
            (!previous ||
              previous.rows.length !== offset ||
              !same(previous.schema, response.schema))) ||
          (response.next_offset !== null &&
            (response.next_offset !== offset + response.rows.length ||
              response.next_offset <= offset ||
              response.next_offset >= 1000)) ||
          offset + response.rows.length > 1000
        )
          throw new Error("The preview page does not match this query.");
        if (!alive()) return;
        lifetime.page = {
          ...response,
          rows:
            offset && previous
              ? [...previous.rows, ...response.rows]
              : response.rows,
        };
        update({ page: lifetime.page, loading: false, running: false });
      } catch (error) {
        fail(error);
      } finally {
        lifetime.paging = false;
      }
    }
    lifetime.more = () => {
      if (lifetime.page?.next_offset != null && !lifetime.paging)
        void page(lifetime.page.next_offset);
    };
    async function poll() {
      const query = lifetime.query;
      if (!query || !alive()) return;
      try {
        const job = (
          await workspace.client.read(
            "ApiQueryV1",
            `/api/v1/queries/${query.id}`,
            { branch: query.context.branch },
            lifetime.abort.signal,
            query.context,
          )
        ).data;
        if (!alive()) return;
        if (
          job.id !== query.id ||
          job.sql !== request?.sql ||
          !same(job.bindings, request.bindings) ||
          !same(job.parameters, request.parameters)
        )
          throw new Error("The preview query receipt changed.");
        if (job.state === "QUEUED" || job.state === "RUNNING") {
          lifetime.timer = setTimeout(() => void poll(), 500);
        } else {
          lifetime.running = false;
          if (job.state === "SUCCEEDED") await page();
          else
            fail(
              new Error(
                job.error ??
                  `Preview query ${job.state.toLowerCase().replaceAll("_", " ")}.`,
              ),
            );
        }
      } catch (error) {
        fail(error);
      }
    }
    update({ page: null, loading: true, running: true, error: "" });
    void (async () => {
      try {
        const commandContext = (
          await workspace.client.read(
            "ApiContextV1",
            "/api/v1/context",
            { branch: context.branch },
            lifetime.abort.signal,
          )
        ).data;
        if (!alive()) return;
        if (
          commandContext.workspace !== context.workspace ||
          commandContext.branch !== context.branch
        )
          throw new Error("The preview workspace or branch changed.");
        // Do not abort acceptance: obtain its owned ID even if the view closes.
        const accepted = await workspace.client.startQuery(
          request,
          commandContext,
          crypto.randomUUID(),
          new AbortController().signal,
        );
        lifetime.query = { id: accepted.id, context: commandContext };
        if (!alive()) {
          cancelOwned();
          return;
        }
        await poll();
      } catch (error) {
        fail(error);
      }
    })();
    return () => {
      lifetime.closed = true;
      clearTimeout(lifetime.timer);
      lifetime.abort.abort();
      cancelOwned();
    };
    // Request/context values form the immutable lifetime key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, key, revision]);
  const current = result?.key === key ? result : null;
  return {
    page: current?.page ?? null,
    loading: !!request && (current?.loading ?? true),
    running: !!request && (current?.running ?? true),
    error: current?.error ?? "",
    more: () => {
      if (!current?.error) owned.current?.more?.();
    },
    restart: () => setRevision((value) => value + 1),
    cancel: () => {
      const lifetime = owned.current;
      if (!lifetime) return;
      lifetime.canceled = true;
      lifetime.abort.abort();
      clearTimeout(lifetime.timer);
      setResult({
        key,
        page: lifetime.page,
        loading: false,
        running: false,
        error: "Preview query canceled.",
      });
      if (lifetime.query && lifetime.running)
        void workspace.client
          .cancelQuery(
            lifetime.query.id,
            lifetime.query.context,
            new AbortController().signal,
          )
          .catch(() => undefined);
    },
  };
}
