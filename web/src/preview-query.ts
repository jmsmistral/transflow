import type {
  ApiQueryRequestV1,
  LogicalSchemaV1,
  LogicalType,
} from "./generated/contracts";

export type PreviewFilter =
  | { column: string; kind: "include" | "exclude"; value: string }
  | { column: string; kind: "null" | "not_null" };
export type PreviewCriteria = {
  filters: readonly PreviewFilter[];
  sort: { column: string; direction: "asc" | "desc" } | null;
};
export const emptyPreviewCriteria: PreviewCriteria = {
  filters: [],
  sort: null,
};

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
  if (criteria.sort) {
    field(criteria.sort.column);
    sql += ` ORDER BY ${identifier(criteria.sort.column)} ${criteria.sort.direction === "asc" ? "ASC" : "DESC"} NULLS LAST`;
  }
  return { sql, parameters };
}
