import type {
  ApiQueryRequestV1,
  LogicalSchemaV1,
  LogicalType,
  ApiPreviewCellV1,
} from "./generated/contracts";

export type PreviewFilter =
  | { column: string; kind: "include" | "exclude"; value: string }
  | { column: string; kind: "null" | "not_null" };
export type PreviewCriteria = {
  filters: readonly PreviewFilter[];
  sorts: readonly { column: string; direction: "asc" | "desc" }[];
};
export const emptyPreviewCriteria: PreviewCriteria = {
  filters: [],
  sorts: [],
};

/** Use the exact wire scalar, never its decorated or rounded display text. */
export function previewCellFilter(
  column: string,
  cell: ApiPreviewCellV1,
  kind: "include" | "exclude",
): PreviewFilter | null {
  const value = cell.value;
  if (cell.truncated || value === null) return null;
  if (value.type === "list" || value.type === "struct") return null;
  if (value.type === "null")
    return { column, kind: kind === "include" ? "null" : "not_null" };
  const text =
    value.type === "binary"
      ? Array.from(
          atob(value.value),
          (byte) => `\\x${byte.charCodeAt(0).toString(16).padStart(2, "0")}`,
        ).join("")
      : String(value.value);
  return { column, kind, value: text };
}

function identifier(name: string) {
  return `"${name.replaceAll('"', '""')}"`;
}

function scalarType(type: LogicalType): string | null {
  switch (type.type) {
    case "bool":
      return "BOOLEAN";
    case "i8":
      return "TINYINT";
    case "i16":
      return "SMALLINT";
    case "i32":
      return "INTEGER";
    case "i64":
      return "BIGINT";
    case "u8":
      return "UTINYINT";
    case "u16":
      return "USMALLINT";
    case "u32":
      return "UINTEGER";
    case "u64":
      return "UBIGINT";
    case "f32":
      return "FLOAT";
    case "f64":
      return "DOUBLE";
    case "decimal":
      return `DECIMAL(${type.precision}, ${type.scale})`;
    case "date":
      return "DATE";
    case "timestamp":
      return type.timezone !== null
        ? "TIMESTAMPTZ"
        : {
            s: "TIMESTAMP_S",
            ms: "TIMESTAMP_MS",
            us: "TIMESTAMP",
            ns: "TIMESTAMP_NS",
          }[type.unit];
    case "binary":
      return "BLOB";
    case "string":
      return "VARCHAR";
    case "list":
    case "struct":
      return null;
  }
}

/** Only schema identifiers enter SQL. Values remain separately bound parameters. */
export function previewQuery(
  schema: LogicalSchemaV1,
  criteria: PreviewCriteria,
): Pick<ApiQueryRequestV1, "sql" | "parameters"> {
  const field = (name: string) => {
    const result = schema.fields.find((f) => f.name === name);
    if (!result) throw new Error("The selected preview column is unavailable.");
    return result;
  };
  const parameters: string[] = [];
  const predicates = criteria.filters.map((filter) => {
    const type = scalarType(field(filter.column).logical_type);
    const column = identifier(filter.column);
    if (!("value" in filter))
      return `${column} IS ${filter.kind === "not_null" ? "NOT " : ""}NULL`;
    parameters.push(filter.value);
    // DISTINCT FROM makes Exclude the complement of Include, retaining nulls.
    const comparison =
      filter.kind === "include" ? "IS NOT DISTINCT FROM" : "IS DISTINCT FROM";
    return type
      ? `${column} ${comparison} CAST(? AS ${type})`
      : `CAST(${column} AS VARCHAR) ${comparison} ?`;
  });
  let sql = "SELECT * FROM dataset";
  if (predicates.length) sql += ` WHERE ${predicates.join(" AND ")}`;
  if (criteria.sorts.length) {
    sql += ` ORDER BY ${criteria.sorts
      .map((sort) => {
        field(sort.column);
        return `${identifier(sort.column)} ${sort.direction === "asc" ? "ASC" : "DESC"} NULLS LAST`;
      })
      .join(", ")}`;
  }
  return { sql, parameters };
}

/** Strip a terminal statement separator without touching quoted SQL or comments.
 * This is presentation composition of an already accepted query, not validation;
 * the coordinator still validates the complete composed SQL with its AST policy.
 */
function withoutTerminalSeparator(sql: string): string {
  let last = -1;
  for (let i = 0; i < sql.length;) {
    const char = sql[i];
    if (/\s/u.test(char ?? "")) {
      i++;
      continue;
    }
    if (sql.startsWith("--", i)) {
      const end = sql.indexOf("\n", i + 2);
      i = end < 0 ? sql.length : end + 1;
      continue;
    }
    if (sql.startsWith("/*", i)) {
      let depth = 1;
      i += 2;
      while (i < sql.length && depth) {
        if (sql.startsWith("/*", i)) {
          depth++;
          i += 2;
        } else if (sql.startsWith("*/", i)) {
          depth--;
          i += 2;
        } else i++;
      }
      continue;
    }
    last = i;
    if (char === "'" || char === '"') {
      const escaped =
        char === "'" &&
        /[eE]/u.test(sql[i - 1] ?? "") &&
        !/[\w$]/u.test(sql[i - 2] ?? "");
      i++;
      while (i < sql.length) {
        if (escaped && sql[i] === "\\") {
          i += 2;
          continue;
        }
        if (sql[i++] === char) {
          if (sql[i] === char) i++;
          else break;
        }
      }
    } else if (
      char === "$" &&
      /^\$(?:[A-Za-z_][A-Za-z_0-9]*)?\$/u.test(sql.slice(i))
    ) {
      const delimiter =
        /^\$(?:[A-Za-z_][A-Za-z_0-9]*)?\$/u.exec(sql.slice(i))?.[0] ?? "$$";
      const end = sql.indexOf(delimiter, i + delimiter.length);
      i = end < 0 ? sql.length : end + delimiter.length;
    } else i++;
  }
  return sql[last] === ";" ? sql.slice(0, last) + sql.slice(last + 1) : sql;
}

export function scratchpadPreviewQuery(
  sql: string,
  schema: LogicalSchemaV1,
  criteria: PreviewCriteria,
) {
  const query = previewQuery(schema, criteria);
  return {
    ...query,
    sql: query.sql.replace(
      "SELECT * FROM dataset",
      () =>
        `SELECT * FROM (\n${withoutTerminalSeparator(sql)}\n) AS "__transflow_result"`,
    ),
  };
}
