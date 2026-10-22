import type { Client, Query } from "./api/client";
import type {
  ApiContextV1,
  ApiDatasetsV1,
  ApiQueryRequestV1,
} from "./generated/contracts";
import { field } from "./inspectors";
import { catalogueSql } from "./sql-catalogue";

export type QueryBinding = ApiQueryRequestV1["bindings"][number];
export type BoundSql = {
  sql: string;
  bindings: readonly QueryBinding[];
  fallbacks: readonly string[];
};

export async function bindCatalogueSql(
  client: Client,
  sql: string,
  selected: QueryBinding,
  context: ApiContextV1,
  query: Query,
  signal: AbortSignal,
  selectedPath?: string,
): Promise<BoundSql> {
  const translated = catalogueSql(sql, selectedPath);
  const bindings: QueryBinding[] = [selected];
  const fallbacks: string[] = [];
  for (const { path, alias } of translated.paths) {
    let cursor: string | null = null;
    const seen = new Set<string>();
    let found = false;
    for (let page = 0; page < 10; page++) {
      const response = await client.read(
        "ApiDatasetsV1",
        "/api/v1/datasets",
        {
          ...query,
          filter: path,
          limit: "200",
          ...(cursor ? { cursor } : {}),
        },
        signal,
        context,
      );
      const data: ApiDatasetsV1 = response.data;
      const entry = data.entries.find(
        (item) => item.path === path && !item.tombstone,
      );
      if (entry) {
        let version = field(entry.head, "version");
        let branch = field(entry.head, "resolved_branch");
        if (!version && entry.origin === "external") {
          const inspection = (
            await client.read(
              "ApiDatasetInspectionV1",
              `/api/v1/datasets/${entry.dataset_id}/inspection`,
              { ...query, origin_workspace: entry.workspace_id },
              signal,
              context,
            )
          ).data;
          if (
            inspection.dataset !== entry.dataset_id ||
            inspection.origin_workspace !== entry.workspace_id
          )
            throw new Error(
              "The catalogue dataset identity changed. Run the query again.",
            );
          version = inspection.suggested_head?.version;
          branch = inspection.suggested_head?.branch;
        }
        if (!version)
          throw new Error(
            `No published version is available for ${path} on branch ${context.branch} or its fallback branches.`,
          );
        bindings.push({
          alias,
          dataset: entry.dataset_id,
          origin_workspace: entry.workspace_id,
          version,
        });
        if (branch && branch !== context.branch)
          fallbacks.push(`${path} from branch ${branch}`);
        found = true;
        break;
      }
      cursor = data.next_cursor;
      if (!cursor) break;
      if (seen.has(cursor))
        throw new Error(
          "Catalogue search did not advance. Run the query again.",
        );
      seen.add(cursor);
    }
    if (!found)
      throw new Error(
        cursor
          ? `Catalogue lookup exceeded its limit for ${path}. Use a more specific dataset path.`
          : `Dataset ${path} was not found in this catalogue. Use its exact quoted path.`,
      );
  }
  return { sql: translated.sql, bindings, fallbacks };
}
