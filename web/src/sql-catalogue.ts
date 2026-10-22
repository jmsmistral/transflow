/** SQL name assistance only. The worker's DuckDB AST policy remains authoritative. */
type Token = {
  start: number;
  end: number;
  value: string;
  kind: "word" | "identifier" | "opaque" | "comment" | "symbol";
  closed?: boolean;
};

function tokens(sql: string): Token[] {
  const result: Token[] = [];
  let i = 0;
  while (i < sql.length) {
    const start = i;
    const char = sql[i] ?? "";
    if (/\s/.test(char)) {
      i++;
      continue;
    }
    if (sql.startsWith("--", i)) {
      i = sql.indexOf("\n", i);
      if (i < 0) i = sql.length;
      result.push({ start, end: i, value: "", kind: "comment" });
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
      result.push({ start, end: i, value: "", kind: "comment" });
      continue;
    }
    const dollar = sql.slice(i).match(/^\$(?:[A-Za-z_][A-Za-z_0-9]*)?\$/)?.[0];
    if (dollar) {
      const end = sql.indexOf(dollar, i + dollar.length);
      i = end < 0 ? sql.length : end + dollar.length;
      result.push({ start, end: i, value: "", kind: "opaque" });
      continue;
    }
    if (char === "'" || char === '"') {
      i++;
      let value = "";
      let closed = false;
      const escaped =
        char === "'" &&
        /[eE]/.test(sql[start - 1] ?? "") &&
        (start < 2 || !/[A-Za-z_0-9]/.test(sql[start - 2] ?? ""));
      while (i < sql.length) {
        if (escaped && sql[i] === "\\") {
          value += sql.slice(i, i + 2);
          i += 2;
          continue;
        }
        if (sql[i] === char) {
          i++;
          if (sql[i] !== char) {
            closed = true;
            break;
          }
        }
        value += sql[i++];
      }
      result.push({
        start,
        end: i,
        value,
        closed,
        kind: char === '"' ? "identifier" : "opaque",
      });
      continue;
    }
    if (/[A-Za-z_]/.test(char)) {
      while (i < sql.length && /[A-Za-z_0-9$]/.test(sql[i] ?? "")) i++;
      result.push({ start, end: i, value: sql.slice(start, i), kind: "word" });
      continue;
    }
    i++;
    result.push({ start, end: i, value: char, kind: "symbol" });
  }
  return result;
}

function relations(all: Token[]) {
  const stack = [{ query: false, from: false, expected: false }];
  const indexes = new Set<number>();
  for (const [index, token] of all.entries()) {
    const frame = stack.at(-1);
    if (!frame || ["opaque", "comment"].includes(token.kind)) continue;
    if (token.value === "(" && token.kind === "symbol") {
      stack.push({
        query: frame.expected,
        from: frame.expected,
        expected: frame.expected,
      });
      frame.expected = false;
      continue;
    }
    if (token.value === ")" && token.kind === "symbol") {
      if (stack.length > 1) stack.pop();
      continue;
    }
    const keyword = token.kind === "word" ? token.value.toUpperCase() : "";
    if (keyword === "SELECT") {
      frame.query = true;
      frame.from = false;
      frame.expected = false;
    } else if (
      (keyword === "FROM" && frame.query) ||
      (keyword === "JOIN" && frame.from)
    ) {
      frame.from = true;
      frame.expected = true;
    } else if (
      /^(WHERE|GROUP|HAVING|ORDER|LIMIT|OFFSET|QUALIFY|WINDOW|UNION|EXCEPT|INTERSECT)$/.test(
        keyword,
      )
    ) {
      frame.from = false;
      frame.expected = false;
    } else if (token.value === "," && token.kind === "symbol" && frame.from)
      frame.expected = true;
    else if (
      frame.expected &&
      (token.kind === "word" || token.kind === "identifier")
    ) {
      indexes.add(index);
      frame.expected = false;
    }
  }
  return { indexes, expected: stack.at(-1)?.expected ?? false };
}

function ctes(all: Token[]) {
  const ends = new Map<number, number>();
  const opens: number[] = [];
  const parents = new Map<number, number>();
  for (const [i, token] of all.entries()) {
    parents.set(i, opens.at(-1) ?? -1);
    if (token.kind !== "symbol") continue;
    if (token.value === "(") opens.push(i);
    if (token.value === ")") {
      const open = opens.pop();
      if (open !== undefined) ends.set(open, i);
    }
  }
  const scopes: { start: number; end: number; name: string }[] = [];
  for (const [i, token] of all.entries()) {
    if (token.kind !== "word" || token.value.toUpperCase() !== "WITH") continue;
    let next = i + 1;
    if (all[next]?.value.toUpperCase() === "RECURSIVE") next++;
    while (next < all.length) {
      const name = all[next];
      if (!name || !["word", "identifier"].includes(name.kind)) break;
      next++;
      if (all[next]?.value === "(") next = (ends.get(next) ?? all.length) + 1;
      if (all[next]?.value.toUpperCase() !== "AS") break;
      next++;
      if (all[next]?.value !== "(") break;
      scopes.push({
        start: i,
        end: ends.get(parents.get(i) ?? -1) ?? all.length,
        name: name.value.toLowerCase(),
      });
      next = (ends.get(next) ?? all.length) + 1;
      if (all[next]?.value !== ",") break;
      next++;
    }
  }
  return scopes;
}

export type SqlCompletion = { start: number; end: number; query: string };
export function sqlCompletion(
  sql: string,
  caret: number,
): SqlCompletion | null {
  const before = tokens(sql.slice(0, caret));
  const { indexes, expected } = relations(before);
  const last = before.at(-1);
  if (last && ["opaque", "comment"].includes(last.kind) && last.end === caret)
    return null;
  if (last && indexes.has(before.length - 1) && last.end === caret) {
    const full = tokens(sql).find((token) => token.start === last.start);
    return { start: last.start, end: full?.end ?? caret, query: last.value };
  }
  return expected ? { start: caret, end: caret, query: "" } : null;
}

export const quoteSqlName = (path: string) => `"${path.replaceAll('"', '""')}"`;

export function catalogueSql(sql: string, selectedPath?: string) {
  if (new TextEncoder().encode(sql).length > 16384)
    throw new Error("SQL is limited to 16 KiB.");
  const all = tokens(sql).filter((token) => token.kind !== "comment");
  if (all.some((token) => token.kind === "identifier" && !token.closed))
    throw new Error("Close the quoted dataset name before running the query.");
  const { indexes } = relations(all);
  const scopes = ctes(all);
  const names = new Set(all.map((token) => token.value.toLowerCase()));
  const paths = new Map<string, string>();
  const replacements = new Map<number, string>();
  for (const index of indexes) {
    const token = all[index];
    if (
      !token ||
      token.kind !== "identifier" ||
      token.value.toLowerCase() === "dataset" ||
      scopes.some(
        (scope) =>
          index > scope.start &&
          index < scope.end &&
          scope.name === token.value.toLowerCase(),
      )
    )
      continue;
    // Schema/catalog-qualified names stay subject to the existing SQL policy.
    if (all[index + 1]?.value === ".") continue;
    if (token.value === selectedPath) {
      replacements.set(index, quoteSqlName("dataset"));
      continue;
    }
    let alias = paths.get(token.value);
    if (!alias) {
      let suffix = paths.size + 1;
      do {
        alias = `__tf_catalogue_${suffix++}`;
      } while (names.has(alias));
      names.add(alias);
      paths.set(token.value, alias);
    }
    replacements.set(index, quoteSqlName(alias));
  }
  if (paths.size > 31)
    throw new Error(
      "A query can use dataset plus at most 31 catalogue datasets.",
    );
  // Preserve quoted column names and string/comment contents. Only table names
  // and their explicitly qualified references are translated to bound views.
  for (const [index, token] of all.entries()) {
    const alias =
      token.kind === "identifier"
        ? token.value === selectedPath
          ? "dataset"
          : paths.get(token.value)
        : undefined;
    if (
      alias &&
      all[index + 1]?.value === "." &&
      !scopes.some(
        (scope) =>
          index > scope.start &&
          index < scope.end &&
          scope.name === token.value.toLowerCase(),
      )
    )
      replacements.set(index, quoteSqlName(alias));
  }
  let translated = sql;
  for (const [index, replacement] of [...replacements].sort(
    ([a], [b]) => b - a,
  )) {
    const token = all[index];
    if (token)
      translated =
        translated.slice(0, token.start) +
        replacement +
        translated.slice(token.end);
  }
  return {
    sql: translated,
    paths: [...paths].map(([path, alias]) => ({ path, alias })),
  };
}
