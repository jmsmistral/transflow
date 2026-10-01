import { useMemo } from "react";
import type { sourceDiff } from "./execution-model";

type Token = { text: string; kind: string };
const pythonWords = new Set(
  "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case".split(
    " ",
  ),
);
const sqlWords = new Set(
  "select from where group by having order asc desc limit offset join left right full inner outer cross on as with recursive union all distinct intersect except case when then else end null true false and or not in is like ilike between exists over partition rows range unbounded preceding following current row create table view insert into values update set delete cast filter qualify".split(
    " ",
  ),
);
/** A display-only lexer. Keep source text intact and let React escape every token. */
export function sourceTokens(text: string, path: string): Token[][] {
  const sql = /\.sql$/i.test(path);
  if (!sql && !/\.py$/i.test(path))
    return text.split("\n").map((text) => [{ text, kind: "plain" }]);
  const pattern = sql
    ? /(--[^\n]*|\/\*[\s\S]*?(?:\*\/|$))|('(?:''|[^'])*(?:'|$)|"(?:""|[^"])*(?:"|$))|(\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b)|([A-Za-z_]\w*)/g
    : /(#[^\n]*)|((?:[rRuUbBfF]{1,2})?(?:"""[\s\S]*?(?:"""|$)|'''[\s\S]*?(?:'''|$)|"(?:\\[\s\S]|[^"\\\n])*(?:"|$)|'(?:\\[\s\S]|[^'\\\n])*(?:'|$)))|(\b(?:0[xX][\da-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?[\d_]+)?j?)\b)|([A-Za-z_]\w*)/g;
  const lines: Token[][] = [[]];
  function append(value: string, kind: string) {
    value.split("\n").forEach((text, i) => {
      if (i) lines.push([]);
      if (text) lines.at(-1)?.push({ text, kind });
    });
  }
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    append(text.slice(offset, match.index), "plain");
    const word = match[4];
    const kind = match[1]
      ? "comment"
      : match[2]
        ? "string"
        : match[3]
          ? "number"
          : word &&
              (sql ? sqlWords.has(word.toLowerCase()) : pythonWords.has(word))
            ? "keyword"
            : "plain";
    append(match[0], kind);
    offset = match.index + match[0].length;
  }
  append(text.slice(offset), "plain");
  return lines;
}
export function SourceLines({
  source,
  current,
  diff,
}: {
  source: { text: string; path: string } | undefined;
  current: { text: string; path: string } | undefined;
  diff: ReturnType<typeof sourceDiff> | null;
}) {
  const before = useMemo(
    () => sourceTokens(source?.text ?? "", source?.path ?? ""),
    [source],
  );
  const after = useMemo(
    () => sourceTokens(current?.text ?? "", current?.path ?? ""),
    [current],
  );
  let oldLine = 0,
    newLine = 0;
  const lines = diff
    ? diff.map((line) => {
        const tokens = line.kind === "added" ? after[newLine] : before[oldLine];
        if (line.kind !== "added") oldLine++;
        if (line.kind !== "removed") newLine++;
        return {
          kind: line.kind,
          tokens: tokens ?? [{ text: line.text, kind: "plain" }],
        };
      })
    : before.map((tokens) => ({ kind: "plain", tokens }));
  return (
    <ol className="source-lines">
      {lines.map((line, i) => (
        <li key={i} className={`diff-${line.kind}`}>
          <code>
            {diff
              ? line.kind === "added"
                ? "+ "
                : line.kind === "removed"
                  ? "− "
                  : "  "
              : ""}
            {line.tokens.length
              ? line.tokens.map((token, j) => (
                  <span key={j} className={`syntax-${token.kind}`}>
                    {token.text}
                  </span>
                ))
              : " "}
          </code>
        </li>
      ))}
    </ol>
  );
}
